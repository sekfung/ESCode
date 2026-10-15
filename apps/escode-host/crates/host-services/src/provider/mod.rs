//! `providers` 域（docs/specs/rust-u2-providers.md；TS 真相源 `packages/provider*`、`services/src/model-provider`）。
//!
//! P1：只读 Registry——从 Built-in（Bundled）与 Personal 两份文件解析出 Registry，提供 `model-selection.getView`。

mod complete;
mod files;
mod issues;
mod legacy;
mod overlay;
mod resolve;
mod rules;
mod selection;
mod settings_view;
mod types;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use escode_cli_host::file_stamp::{ChangeGate, FileStamp};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex as AsyncMutex;

use files::{Builtin, Personal};
use legacy::{LegacyContext, resolve_legacy_level};
use resolve::{
    Input, ProviderMap, RegistryProvider, Resolution, fail_closed_account, resolve_registry,
};
use rules::Rule;
use selection::{Selection, classify, project_provider, resolve_effective, resolve_initial};

/// 一次成功加载的快照。
struct Loaded {
    /// 两份文件的规范化指纹（解析后再序列化，格式/键序不同的等价文件指纹相同）。
    fingerprint: String,
    revision: u64,
    registry: Vec<RegistryProvider>,
    effective: ProviderMap,
    builtin_rules: Vec<Rule>,
    personal_rules: Vec<Rule>,
    default_selection: Option<Selection>,
    resolved: Vec<resolve::ResolvedProvider>,
    templates: Vec<resolve::Template>,
    personal_providers: ProviderMap,
}

/// 指纹不变时两次真读之间的最长间隔（兜底：Windows 没有 inode/ctime，同粒度原地写可能骗过指纹）。
const FULL_READ_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

struct State {
    gate: ChangeGate,
    loaded: Option<Arc<Loaded>>,
    /// Registry 替换次数（TS `ProviderRegistry.replace` 递增的修订号）。
    revision: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            gate: ChangeGate::new(FULL_READ_EVERY),
            loaded: None,
            revision: 0,
        }
    }
}

pub struct ProviderService {
    builtin_path: PathBuf,
    personal_path: PathBuf,
    /// 串行化加载（文件读取 + 解析 + 替换），避免并发请求重复推进修订号。
    gate: AsyncMutex<()>,
    state: Mutex<State>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 规范化字符串：解析后再序列化（`serde_json::Map` 按键排序）。
fn canonical(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

impl ProviderService {
    pub fn new(builtin_path: PathBuf, personal_path: PathBuf) -> Self {
        Self {
            builtin_path,
            personal_path,
            gate: AsyncMutex::new(()),
            state: Mutex::default(),
        }
    }

    /// TS `ensureReady()` + 读取：文件指纹变化才重新解析并推进修订号。
    async fn load(&self) -> Result<Arc<Loaded>> {
        let _guard = self.gate.lock().await;
        // 文件指纹未变（且未到兜底间隔）就复用上次解析结果，不再读盘/解析（P1 #7 同一机制）。
        // 指纹在读之前取：读到一半发生的变化会体现在下一轮指纹里。
        let stamps = vec![
            FileStamp::of(&self.builtin_path).await,
            FileStamp::of(&self.personal_path).await,
        ];
        {
            let state = lock(&self.state);
            if let Some(loaded) = &state.loaded
                && !state.gate.should_read(&stamps, std::time::Instant::now())
            {
                return Ok(Arc::clone(loaded));
            }
        }
        let builtin_text = tokio::fs::read_to_string(&self.builtin_path)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "Bundled 与 Active ESCode Built-in Release 均不可用: {}: {error}",
                    self.builtin_path.display()
                )
            })?;
        let builtin_value: Value = serde_json::from_str(&builtin_text)?;
        // Personal：不存在 → 空配置；无效 → 原样保留文件、内存空配置降级（TS `recoverInvalidFile`）。
        let personal_value = match tokio::fs::read_to_string(&self.personal_path).await {
            Ok(text) => serde_json::from_str::<Value>(&text).ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let fingerprint = format!(
            "{}\n{}",
            canonical(&builtin_value),
            personal_value
                .as_ref()
                .map_or_else(|| "none".to_owned(), canonical)
        );
        // 先把 Arc 取出再判断：`if let` 的临时 guard 会活到整个分支结束，分支里再取锁会自锁。
        let cached = lock(&self.state).loaded.clone();
        if let Some(loaded) = cached
            && loaded.fingerprint == fingerprint
        {
            // 内容没变（只是元数据变了，如 touch / 重写同内容）：不推进修订号，但记下新指纹。
            lock(&self.state)
                .gate
                .record(stamps, std::time::Instant::now());
            return Ok(loaded);
        }
        let builtin: Builtin = files::parse_builtin(&builtin_value)?;
        let personal: Personal = match &personal_value {
            None => Personal::default(),
            Some(value) => files::parse_personal(value).unwrap_or_else(|error| {
                eprintln!(
                    "escode-host: personal provider config invalid, using empty overlay: {error:#}"
                );
                Personal::default()
            }),
        };
        let account: ProviderMap = fail_closed_account(&builtin.providers);
        let no_state = |_: &str| None;
        let Resolution {
            registry,
            effective,
            resolved,
        } = resolve_registry(&Input {
            builtin_providers: &builtin.providers,
            templates: &builtin.templates,
            personal_providers: &personal.providers,
            builtin_rules: &builtin.rules,
            personal_rules: &personal.rules,
            account_providers: &account,
            account_current: &no_state,
            personal_provider_order: &personal.provider_order,
        });
        let mut state = lock(&self.state);
        state.revision += 1;
        let loaded = Arc::new(Loaded {
            fingerprint,
            revision: state.revision,
            registry,
            effective,
            builtin_rules: builtin.rules,
            personal_rules: personal.rules,
            default_selection: personal.default_selection,
            resolved,
            templates: builtin.templates,
            personal_providers: personal.providers,
        });
        state.loaded = Some(Arc::clone(&loaded));
        state.gate.record(stamps, std::time::Instant::now());
        Ok(loaded)
    }

    /// TS `ModelSelectionService.getView(input?)`。
    pub async fn model_selection_view(&self, input: Option<&Value>) -> Result<Value> {
        let loaded = self.load().await?;
        let legacy = |selection: &Selection| {
            resolve_legacy_level(
                &LegacyContext {
                    personal_rules: &loaded.personal_rules,
                    builtin_rules: &loaded.builtin_rules,
                    effective: &loaded.effective,
                },
                selection,
            )
        };
        let no_current: Vec<String> = Vec::new();
        // 默认偏好只归一化档位，不借此改写账号身份或保存配置。
        let normalized_default = loaded.default_selection.as_ref().and_then(|default| {
            resolve_effective(
                Some(default),
                &loaded.registry,
                &no_current,
                &|_| selection::ProviderKind::Ordinary,
                Some(&legacy),
            )
            .selection
        });
        let preferred = resolve_initial(&loaded.registry, normalized_default.as_ref());
        let providers: Vec<Value> = loaded
            .registry
            .iter()
            .filter(|provider| {
                provider.config.get("visibility").and_then(Value::as_str) != Some("hidden")
            })
            .map(project_provider)
            .collect();
        let mut view = Map::new();
        view.insert("revision".into(), json!(loaded.revision));
        view.insert("providers".into(), Value::Array(providers));
        if let Some(preferred) = preferred {
            view.insert("preferredSelection".into(), preferred.to_value());
        }
        if let Some(input) = input.filter(|value| value.is_object()) {
            let requested = input
                .get("selection")
                .filter(|v| !v.is_null())
                .and_then(Selection::from_value);
            let effective = resolve_effective(
                requested.as_ref(),
                &loaded.registry,
                &no_current,
                &classify,
                Some(&legacy),
            );
            view.insert(
                "effectiveSelection".into(),
                effective.selection.map_or(Value::Null, |s| s.to_value()),
            );
            if let Some(issue) = effective.issue {
                view.insert("selectionIssue".into(), issue.into());
            }
        }
        Ok(Value::Object(view))
    }

    /// TS `ProviderSettingsService.getView()`（`refresh` 同形：重读文件后返回最新视图）。
    pub async fn provider_settings_view(&self) -> Result<Value> {
        let loaded = self.load().await?;
        Ok(settings_view::build(&settings_view::ViewInput {
            revision: loaded.revision,
            templates: &loaded.templates,
            personal_providers: &loaded.personal_providers,
            personal_rules: &loaded.personal_rules,
            registry_provider_ids: loaded
                .registry
                .iter()
                .map(|p| p.provider_id.as_str())
                .collect(),
            resolved: &loaded.resolved,
        }))
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod service_tests;
