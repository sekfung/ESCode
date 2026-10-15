//! `ProviderService` 的 Personal 写入面（TS `ProviderSettingsFacade` 的 mutate* + `ProviderConfigService`）。
//!
//! 流程与 TS 一致：持有写闸门 → 读最新 Registry 快照算出 Host 侧成员事实（`Membership`）→
//! 文件锁内对最新 Personal 文档执行变更并落盘 → 重新加载并返回最新 Settings 视图。
//! 写闸门是全局串行（TS 是按 Provider 串行；跨进程另有文件锁），不改变可观察的单 Provider 顺序。

use anyhow::{Result, anyhow};
use escode_cli_host::file_stamp::ChangeGate;
use serde_json::{Value, json};

use super::mutations::{self, Builtin, Membership, Metadata};
use super::mutations_create::{self, CreateInput};
use super::mutations_models::{self as models, DraftInput};
use super::personal::Doc;
use super::store::{self, MutationError, Write};
use super::{FULL_READ_EVERY, Loaded, ProviderService, lock};

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

impl ProviderService {
    /// TS `#modelMembership`：配置成员 ≠ 可执行模型，禁用/无权益/不完整的模型仍可编辑。
    fn membership(loaded: &Loaded, provider_id: &str) -> Result<Membership> {
        let provider = loaded
            .resolved
            .iter()
            .find(|provider| provider.provider_id == provider_id)
            .ok_or_else(|| anyhow!("Provider 不存在: {provider_id}"))?;
        Ok(Membership {
            provider_id: provider_id.to_owned(),
            inherited: provider
                .models
                .iter()
                .filter(|model| model.builtin)
                .map(|model| model.model_id.clone())
                .collect(),
            personal_revision: loaded.personal_revision.clone(),
        })
    }

    /// 文件锁内的读-改-写；成功后让文件指纹闸门失效，保证紧随其后的 `load()` 必读新内容。
    async fn commit<T>(
        &self,
        loaded: &Loaded,
        transform: impl FnOnce(&mut Doc, &Builtin<'_>) -> Result<(Write, T), MutationError>,
    ) -> Result<T> {
        let builtin = Builtin {
            providers: &loaded.builtin_providers,
            templates: &loaded.templates,
        };
        let value = store::update(&self.personal_path, |doc| transform(doc, &builtin)).await?;
        lock(&self.state).gate = ChangeGate::new(FULL_READ_EVERY);
        Ok(value)
    }

    /// 单 Provider 变更的公共外壳：写闸门 → 快照/成员 → 变更 → 最新视图。
    async fn mutate_provider<T>(
        &self,
        provider_id: &str,
        transform: impl FnOnce(&mut Doc, &Builtin<'_>, &Membership) -> Result<T, MutationError>,
    ) -> Result<(T, Value)> {
        let _write = self.write_gate.lock().await;
        let loaded = self.load().await?;
        let membership = Self::membership(&loaded, provider_id)?;
        let value = self
            .commit(&loaded, |doc, builtin| {
                transform(doc, builtin, &membership).map(|value| (Write::Commit, value))
            })
            .await?;
        Ok((value, self.provider_settings_view().await?))
    }

    pub async fn save_personal_provider_overlay(
        &self,
        provider_id: &str,
        config: &Value,
        metadata: Option<&Value>,
    ) -> Result<Value> {
        let metadata = Metadata {
            provider_name: metadata.and_then(|m| m.get("providerName")).cloned(),
            template_id: metadata.and_then(|m| m.get("templateId")).cloned(),
            enabled: metadata.and_then(|m| m.get("enabled")).cloned(),
        };
        let ((), view) = self
            .mutate_provider(provider_id, |doc, builtin, membership| {
                mutations::save_provider_overlay(
                    doc,
                    builtin,
                    provider_id,
                    config,
                    membership,
                    &metadata,
                )
            })
            .await?;
        Ok(view)
    }

    pub async fn reorder_personal_models(
        &self,
        provider_id: &str,
        model_ids: &Value,
    ) -> Result<Value> {
        let requested = strings(model_ids);
        let ((), view) = self
            .mutate_provider(provider_id, |doc, builtin, membership| {
                mutations::reorder_models(doc, builtin, provider_id, &requested, membership)
            })
            .await?;
        Ok(view)
    }

    pub async fn add_personal_model(
        &self,
        provider_id: &str,
        model_id: &str,
        config: &Value,
        use_recommended: Option<bool>,
    ) -> Result<Value> {
        let ((), view) = self
            .mutate_provider(provider_id, |doc, builtin, membership| {
                models::add_model(
                    doc,
                    builtin,
                    provider_id,
                    model_id,
                    config,
                    membership,
                    use_recommended,
                )
            })
            .await?;
        Ok(view)
    }

    pub async fn rename_personal_model(
        &self,
        provider_id: &str,
        current: &str,
        next: &str,
    ) -> Result<Value> {
        let _write = self.write_gate.lock().await;
        let loaded = self.load().await?;
        let membership = Self::membership(&loaded, provider_id)?;
        self.commit(&loaded, |doc, _| {
            models::rename_model(doc, provider_id, current, next, &membership)
                .map(|write| (write, ()))
        })
        .await?;
        self.provider_settings_view().await
    }

    pub async fn delete_personal_model(&self, provider_id: &str, model_id: &str) -> Result<Value> {
        let ((), view) = self
            .mutate_provider(provider_id, |doc, _, membership| {
                models::delete_model(doc, provider_id, model_id, membership)
            })
            .await?;
        Ok(view)
    }

    pub async fn set_personal_model_enabled(
        &self,
        provider_id: &str,
        model_id: &str,
        enabled: &Value,
    ) -> Result<Value> {
        let ((), view) = self
            .mutate_provider(provider_id, |doc, _, membership| {
                models::set_model_enabled(doc, provider_id, model_id, enabled, membership)
            })
            .await?;
        Ok(view)
    }

    /// `SavePersonalModelDraftInput`：先比 Registry 修订号（Facade），再在文件锁内比 Personal 版本（Service）。
    pub async fn save_personal_model_draft(&self, input: &Value) -> Result<Value> {
        let provider_id = text(input, "providerId");
        let _write = self.write_gate.lock().await;
        let loaded = self.load().await?;
        let based_on = input.get("basedOnRevision").and_then(Value::as_u64);
        if based_on != Some(loaded.revision) {
            return Err(anyhow!(
                "Provider Settings revision conflict: expected {}, current {}",
                input
                    .get("basedOnRevision")
                    .map_or_else(|| "undefined".to_owned(), Value::to_string),
                loaded.revision
            ));
        }
        let membership = Self::membership(&loaded, provider_id)?;
        let config = input.get("personalConfig").cloned().unwrap_or(Value::Null);
        let draft = DraftInput {
            provider_id,
            original_id: text(input, "originalModelId"),
            next_id: text(input, "nextModelId"),
            config: &config,
            expected_revision: &loaded.personal_revision,
            use_recommended: input.get("useRecommendedConfig").and_then(Value::as_bool),
        };
        self.commit(&loaded, |doc, _| {
            models::save_model_draft(doc, &draft, &membership).map(|()| (Write::Commit, ()))
        })
        .await?;
        self.provider_settings_view().await
    }

    pub async fn delete_personal_provider(&self, provider_id: &str) -> Result<Value> {
        let _write = self.write_gate.lock().await;
        let loaded = self.load().await?;
        self.commit(&loaded, |doc, _| {
            mutations::delete_provider(doc, provider_id).map(|()| (Write::Commit, ()))
        })
        .await?;
        self.provider_settings_view().await
    }

    pub async fn reorder_personal_providers(&self, provider_ids: &Value) -> Result<Value> {
        let requested = strings(provider_ids);
        let _write = self.write_gate.lock().await;
        let loaded = self.load().await?;
        self.commit(&loaded, |doc, _| {
            mutations::reorder_providers(doc, &requested);
            Ok((Write::Commit, ()))
        })
        .await?;
        self.provider_settings_view().await
    }

    /// 返回 `{providerId, view}`。
    pub async fn create_personal_provider(&self, input: Option<&Value>) -> Result<Value> {
        let field = |key: &str| {
            input
                .and_then(|input| input.get(key))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let create = CreateInput {
            template_id: field("templateId"),
            provider_name: field("providerName"),
            locale: field("locale"),
            initial_config: input
                .and_then(|input| input.get("initialConfig"))
                .filter(|config| config.is_object())
                .cloned(),
        };
        let _write = self.write_gate.lock().await;
        let loaded = self.load().await?;
        let provider_id = self
            .commit(&loaded, |doc, builtin| {
                mutations_create::create_provider(doc, builtin, &create)
                    .map(|id| (Write::Commit, id))
            })
            .await?;
        let view = self.provider_settings_view().await?;
        Ok(json!({"providerId": provider_id, "view": view}))
    }
}
