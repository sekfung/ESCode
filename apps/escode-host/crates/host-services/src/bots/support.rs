//! bots 服务共用的 JSON/路径/时间助手。

use std::path::Path;

use serde_json::{Value, json};

pub(super) fn normalize_config(value: Value) -> Result<Value, String> {
    let mut object = value.as_object().cloned().ok_or("Invalid bots config.")?;
    if object.get("version").and_then(Value::as_u64) != Some(3)
        || !object.get("bots").is_some_and(Value::is_array)
    {
        return Err("Invalid bots config.".to_owned());
    }
    object
        .entry("bots".to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    Ok(Value::Object(object))
}

pub(super) fn read_settings(home: &Path) -> Value {
    let path = home.join(".escode").join("v2").join("setting.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

pub(super) fn read_optional_json(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn workspace_ref(path: &str, identity: Option<&str>) -> Value {
    let id = identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(path);
    let label = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    let mut reference = json!({"id": id, "label": label, "workspacePath": path});
    if let Some(identity) = identity {
        reference["workspaceIdentity"] = Value::String(identity.to_owned());
    }
    reference
}

pub(super) fn is_feishu(provider: &str) -> bool {
    matches!(provider, "feishu" | "lark")
}

pub(super) fn bind_code() -> String {
    let bytes = uuid::Uuid::new_v4();
    bytes.simple().to_string()[..6].to_uppercase()
}

pub(super) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) async fn blocking<T, F>(run: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| error.to_string())?
}
