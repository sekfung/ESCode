//! bots 配置/状态管理子集（docs/specs/rust-u2-bots.md）。
//!
//! 只实现 Host 自足的本地 JSON 配置/状态与绑定码；平台 provider、入站消息、群投递、
//! 附件与 V4 流仍是显式缺口。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use super::support::{
    bind_code, blocking, is_feishu, normalize_config, now, read_optional_json, read_settings,
    workspace_ref,
};

const CONFIG_FILE: &str = "bot-config.v3.json";
const STATE_FILE: &str = "bot-state.v3.json";
const BIND_CODE_TTL_MS: i64 = 30_000;

#[derive(Clone)]
pub struct BotsService {
    data_base_dir: PathBuf,
    home: PathBuf,
    write_lock: Arc<Mutex<()>>,
    bind_codes: Arc<Mutex<BTreeMap<String, Value>>>,
}

impl BotsService {
    pub fn new(data_base_dir: PathBuf, home: PathBuf) -> Self {
        Self {
            data_base_dir,
            home,
            write_lock: Arc::default(),
            bind_codes: Arc::default(),
        }
    }

    fn config_path(&self) -> PathBuf {
        self.data_base_dir
            .join(".escode")
            .join("v2")
            .join(CONFIG_FILE)
    }

    fn state_path(&self) -> PathBuf {
        self.data_base_dir
            .join(".escode")
            .join("v2")
            .join(STATE_FILE)
    }

    pub async fn get_config(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || service.read_config()).await
    }

    pub async fn save_config(&self, config: Value) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let config = normalize_config(config)?;
            service.write_json(&service.config_path(), &config)?;
            Ok(config)
        })
        .await
    }

    pub async fn list_bots(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let config = service.read_config()?;
            Ok(config["bots"].clone())
        })
        .await
    }

    pub async fn save_bot(&self, params: Value) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let mut config = service.read_config()?;
            let raw = params
                .get("bot")
                .cloned()
                .ok_or("Bot config is required.")?;
            let mut bot = raw
                .as_object()
                .cloned()
                .ok_or("Bot config must be an object.")?;
            let id = bot
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or("Bot id is required.")?
                .to_owned();
            let name = bot
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            bot.insert("id".to_owned(), Value::String(id.clone()));
            bot.insert("name".to_owned(), Value::String(name));
            if let Some(credential) = params
                .get("credentialValue")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let _ = credential;
                bot.insert(
                    "credentialRef".to_owned(),
                    Value::String(format!("bot:{id}:credential")),
                );
            }
            if let Some(secret) = params
                .get("webhookSecretValue")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let _ = secret;
                bot.insert(
                    "webhookSecretRef".to_owned(),
                    Value::String(format!("bot:{id}:webhook-secret")),
                );
            }
            let bot = Value::Object(bot);
            let bots = config["bots"]
                .as_array_mut()
                .ok_or("Invalid bots config.")?;
            bots.retain(|item| item["id"].as_str() != Some(id.as_str()));
            bots.push(bot.clone());
            service.write_json(&service.config_path(), &config)?;
            Ok(bot)
        })
        .await
    }

    pub async fn remove_bot_secret(&self, bot_id: String) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let mut config = service.read_config()?;
            let bots = config["bots"]
                .as_array_mut()
                .ok_or("Invalid bots config.")?;
            let Some(bot) = bots
                .iter_mut()
                .find(|bot| bot["id"].as_str() == Some(&bot_id))
            else {
                return Err(format!("Bot not found: {bot_id}"));
            };
            let Some(object) = bot.as_object_mut() else {
                return Err("Invalid bot config.".to_owned());
            };
            for key in [
                "credentialRef",
                "webhookSecretRef",
                "providerUserId",
                "displayName",
            ] {
                object.remove(key);
            }
            if object
                .get("provider")
                .and_then(Value::as_str)
                .is_some_and(is_feishu)
            {
                object.remove("feishuAppId");
            }
            let bot = bot.clone();
            service.write_json(&service.config_path(), &config)?;
            let mut state = service.read_state()?;
            if let Some(bots) = state["bots"].as_object_mut() {
                bots.remove(&bot_id);
            }
            service.write_json(&service.state_path(), &state)?;
            Ok(bot)
        })
        .await
    }

    pub async fn delete_bot(&self, bot_id: String) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let mut config = service.read_config()?;
            if let Some(bots) = config["bots"].as_array_mut() {
                bots.retain(|bot| bot["id"].as_str() != Some(bot_id.as_str()));
            }
            service.write_json(&service.config_path(), &config)?;
            let mut state = service.read_state()?;
            if let Some(bots) = state["bots"].as_object_mut() {
                bots.remove(&bot_id);
            }
            service.write_json(&service.state_path(), &state)?;
            Ok(Value::Null)
        })
        .await
    }

    pub async fn get_status(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let config = service.read_config()?;
            let state = service.read_state()?;
            let bots = config["bots"].as_array().cloned().unwrap_or_default();
            let bot_runtime: Vec<Value> = bots
                .iter()
                .map(|bot| {
                    let bot_id = bot["id"].as_str().unwrap_or_default();
                    let enabled = bot["enabled"].as_bool().unwrap_or(false);
                    let mut runtime = json!({
                        "botId": bot_id,
                        "provider": bot["provider"],
                        "status": if enabled { "idle" } else { "disabled" },
                        "message": if enabled { "Bot is configured." } else { "Bot is disabled." },
                    });
                    if let Some(offset) = state["bots"][bot_id]["telegramOffset"].as_i64() {
                        runtime["offset"] = Value::from(offset);
                    }
                    runtime
                })
                .collect();
            Ok(json!({
                "botsCount": bots.len(),
                "enabledBotsCount": bots.iter().filter(|bot| bot["enabled"].as_bool() == Some(true)).count(),
                "contextsCount": state["bots"].as_object().map_or(0, Map::len),
                "botRuntime": bot_runtime,
            }))
        })
        .await
    }

    pub async fn get_bot_states(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let state = service.read_state()?;
            let values = state["bots"]
                .as_object()
                .map(|bots| bots.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            Ok(Value::Array(values))
        })
        .await
    }

    pub async fn reset_bot_state(&self, context_key: String) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let mut state = service.read_state()?;
            if let Some(bots) = state["bots"].as_object_mut() {
                bots.remove(&context_key);
            }
            service.write_json(&service.state_path(), &state)?;
            Ok(Value::Null)
        })
        .await
    }

    pub async fn create_bind_code(&self, params: Value) -> Result<Value, String> {
        let bot_id = params
            .get("botId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or("Bot id is required.")?
            .to_owned();
        let config = self.read_config()?;
        if !config["bots"].as_array().is_some_and(|bots| {
            bots.iter()
                .any(|bot| bot["id"].as_str() == Some(bot_id.as_str()))
        }) {
            return Err(format!("Bot not found: {bot_id}"));
        }
        let code = bind_code();
        let ttl = params
            .get("ttlMs")
            .and_then(Value::as_i64)
            .unwrap_or(BIND_CODE_TTL_MS);
        let expires_at = now() + ttl;
        let allowed = params
            .get("allowedWorkspaces")
            .cloned()
            .unwrap_or_else(|| json!(["*"]));
        self.bind_codes.lock().unwrap().insert(
            code.clone(),
            json!({
                "botId": bot_id,
                "code": code,
                "allowedWorkspaces": allowed,
                "expiresAt": expires_at,
            }),
        );
        Ok(json!({"code": code, "expiresAt": expires_at}))
    }

    pub async fn list_workspace_refs(&self, params: Value) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let current = params.get("currentWorkspace").cloned();
            let mut refs: BTreeMap<String, Value> = BTreeMap::new();
            if let Some(workspace) = current.and_then(|value| value.as_object().cloned())
                && let (Some(path), identity) = (
                    workspace.get("workspacePath").and_then(Value::as_str),
                    workspace
                        .get("workspaceIdentity")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                )
            {
                let reference = workspace_ref(path, identity);
                refs.insert(
                    reference["id"].as_str().unwrap_or_default().to_owned(),
                    reference,
                );
            }
            let settings = read_settings(&service.home);
            for entry in settings["lastWorkspaceSession"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(path) = entry["workspacePath"].as_str() {
                    let identity = if entry["kind"].as_str() == Some("remote") {
                        entry["workspaceIdentity"]
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                    } else {
                        None
                    };
                    let reference = workspace_ref(path, identity);
                    refs.insert(
                        reference["id"].as_str().unwrap_or_default().to_owned(),
                        reference,
                    );
                }
            }
            Ok(Value::Array(refs.into_values().collect()))
        })
        .await
    }

    pub async fn sync_app_runtime_preferences(&self) -> Result<Value, String> {
        // Host 没有为 Bot 建立远端连接；同步只对已有远端 runtime 生效，因而这里是 no-op。
        Ok(Value::Null)
    }

    fn read_config(&self) -> Result<Value, String> {
        read_optional_json(&self.config_path())?
            .map(normalize_config)
            .transpose()
            .map(|value| value.unwrap_or_else(|| json!({"version": 3, "bots": []})))
    }

    fn read_state(&self) -> Result<Value, String> {
        read_optional_json(&self.state_path())?
            .map(|value| {
                value
                    .as_object()
                    .cloned()
                    .map(Value::Object)
                    .ok_or_else(|| "Invalid bots state.".to_owned())
            })
            .transpose()
            .map(|value| value.unwrap_or_else(|| json!({"version": 3, "bots": {}})))
    }

    fn write_json(&self, path: &Path, value: &Value) -> Result<(), String> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
        );
        escode_host_core::atomic_write::write_secret(path, text.as_bytes())
            .map_err(|error| format!("{error:#}"))
    }
}
