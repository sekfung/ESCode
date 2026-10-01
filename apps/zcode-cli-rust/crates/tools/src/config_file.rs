//! 用户 / 工作区配置文件的「读 → 补丁 → 原子写」，对齐 TS
//! `adapters/src/config/file-config.adapter.ts`（`readJsonConfigFileOrEmpty` + `atomicWriteJson`）。
//!
//! 用保序的 `json_order::Json` 而不是 `serde_json::Value`：TS 是 `JSON.parse` → 展开补丁 →
//! `JSON.stringify(value, null, 2)`，用户文件里其余 key 的顺序原样保留、被补丁的 key 落在末尾；
//! 本仓库的 serde_json 没开 `preserve_order`，会把整份用户配置重排。

use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// TS `readJsonConfigFileOrEmpty`：文件不存在视为 `{}`；读失败 / 非 JSON / 非对象分别报错。
pub(super) async fn read_object_or_empty(path: &Path) -> Result<Json> {
    let content = match tokio::fs::read(path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Json::object()),
        Err(e) => {
            return Err(
                anyhow!(e).context(format!("Unable to read config file: {}", path.display()))
            );
        }
    };
    let parsed: Json = serde_json::from_slice(&content).map_err(|e| {
        anyhow!(e).context(format!(
            "Unable to parse config file as JSON: {}",
            path.display()
        ))
    })?;
    if !parsed.is_object() {
        bail!("Config file must contain a JSON object: {}", path.display());
    }
    Ok(parsed)
}

/// TS `atomicWriteJson`：同目录临时文件（unix 0600）+ rename，内容为
/// `JSON.stringify(value, null, 2) + "\n"`；失败时清理临时文件。
pub(super) async fn atomic_write(path: &Path, value: &Json) -> Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let temp = directory.join(format!(
        ".{file_name}.{}.{millis}.{:x}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let content = format!("{}\n", value.pretty());
    let write = async {
        tokio::fs::create_dir_all(directory).await?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temp).await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, content.as_bytes()).await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        drop(file);
        tokio::fs::rename(&temp, path).await
    };
    if let Err(error) = write.await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(
            anyhow!(error).context(format!("Unable to write config file: {}", path.display()))
        );
    }
    Ok(())
}

const LEGACY_CUA_PLUGIN_ID: &str = "zcode-cua@zcode-plugins-official";
const CANONICAL_CUA_PLUGIN_ID: &str = "computer-use@zcode-plugins-official";

/// TS `isRecord(x) ? x : {}`。
fn object_or_empty(value: Option<&Json>) -> Json {
    value
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(Json::object)
}

/// TS `patchPluginEnabled`：只改 `plugins.enabledPlugins[id]`，先删掉 id 的别名（CUA 旧 id），
/// 再把规范 id 写到 enabledPlugins 末尾；文件其余内容与顺序不动。
pub(super) fn patch_plugin_enabled(root: &mut Json, plugin_id: &str, enabled: bool) {
    // TS `canonicalizePluginId` / `pluginIdAliases`。
    let canonical = if plugin_id == LEGACY_CUA_PLUGIN_ID {
        CANONICAL_CUA_PLUGIN_ID
    } else {
        plugin_id
    };
    let mut plugins = object_or_empty(root.get("plugins"));
    let mut enabled_plugins = object_or_empty(plugins.get("enabledPlugins"));
    enabled_plugins.remove(canonical);
    if canonical == CANONICAL_CUA_PLUGIN_ID {
        enabled_plugins.remove(LEGACY_CUA_PLUGIN_ID);
    }
    enabled_plugins.set(canonical, Json::Bool(enabled));
    plugins.set("enabledPlugins", enabled_plugins);
    root.set("plugins", plugins);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Json {
        Json::parse(text).unwrap()
    }

    #[test]
    fn patch_keeps_order_and_appends_canonical_id() {
        let mut root = parse(
            r#"{"z":1,"plugins":{"dirs":["x"],"enabledPlugins":{"a@m":true,"zcode-cua@zcode-plugins-official":false,"b@m":false}},"y":2}"#,
        );
        patch_plugin_enabled(&mut root, "a@m", false);
        patch_plugin_enabled(&mut root, "zcode-cua@zcode-plugins-official", true);
        assert_eq!(
            root.compact(),
            r#"{"z":1,"plugins":{"dirs":["x"],"enabledPlugins":{"b@m":false,"a@m":false,"computer-use@zcode-plugins-official":true}},"y":2}"#
        );
    }

    #[test]
    fn patch_replaces_non_object_plugins_in_place() {
        let mut root = parse(r#"{"plugins":null,"x":1}"#);
        patch_plugin_enabled(&mut root, "p@m", true);
        assert_eq!(
            root.compact(),
            r#"{"plugins":{"enabledPlugins":{"p@m":true}},"x":1}"#
        );
    }

    #[tokio::test]
    async fn read_missing_is_empty_and_write_matches_json_stringify() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");
        let mut root = read_object_or_empty(&path).await.unwrap();
        assert_eq!(root, Json::object());
        patch_plugin_enabled(&mut root, "p@m", true);
        atomic_write(&path, &root).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"plugins\": {\n    \"enabledPlugins\": {\n      \"p@m\": true\n    }\n  }\n}\n"
        );
        // 临时文件已 rename 走，目录里只剩目标文件。
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[tokio::test]
    async fn read_rejects_non_object_and_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "[1]").unwrap();
        let error = read_object_or_empty(&path).await.unwrap_err().to_string();
        assert!(
            error.starts_with("Config file must contain a JSON object"),
            "{error}"
        );
        std::fs::write(&path, "{").unwrap();
        let error = read_object_or_empty(&path).await.unwrap_err().to_string();
        assert!(
            error.starts_with("Unable to parse config file as JSON"),
            "{error}"
        );
    }
}
