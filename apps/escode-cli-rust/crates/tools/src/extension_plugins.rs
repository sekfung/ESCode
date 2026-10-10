use super::extension_config::{json_file, resolve, storage, strings};
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub(super) struct Plugin {
    pub id: String,
    pub root: PathBuf,
    pub name: String,
    pub manifest: Value,
    pub official: bool,
    /// `plugins.list` 需要的市场/来源/启用态（`enabled()` 之外的新读面，见 docs/specs/rust-plugins.md）。
    pub marketplace: String,
    pub source: &'static str,
    pub enabled: bool,
}
pub(super) async fn enabled(
    cwd: &Path,
    config: &Value,
    cancel: &CancellationToken,
) -> Result<Vec<Plugin>> {
    Ok(all(cwd, config, cancel)
        .await?
        .into_iter()
        .filter(|plugin| plugin.enabled)
        .collect())
}

/// 全部已发现的插件（含被 `enabledPlugins` 关闭的），`enabled` 只作为标志返回。
pub(super) async fn all(
    cwd: &Path,
    config: &Value,
    cancel: &CancellationToken,
) -> Result<Vec<Plugin>> {
    Ok(discover(cwd, config, cancel).await?.0)
}

/// `all` + 发现层诊断（TS `discoverNodePluginsSync` 的 loader 部分：根/manifest 缺失与非法、重复 id、
/// 仅诊断的组件键）。协议形状同 TS `toPluginDiagnostic`。
pub(super) async fn discover(
    cwd: &Path,
    config: &Value,
    cancel: &CancellationToken,
) -> Result<(Vec<Plugin>, Vec<Value>)> {
    let mut diagnostics = vec![];
    if config["plugins"]["enabled"] == false {
        return Ok((vec![], diagnostics));
    }
    let storage = storage(config);
    let mut candidates = strings(&config["plugins"]["dirs"])
        .into_iter()
        .map(|p| (resolve(cwd, p), "inline".to_owned(), "inline", true, false))
        .collect::<Vec<_>>();
    let official = "escode-plugins-official";
    // 修复：Rust 此前只读缓存，从未 seed 随包官方插件；仅运行 Rust 的环境因此没有任何官方插件与技能。
    // 与 TS resolveOfficialPluginRoots 一样在发现前 seed（进程内每个 storage 一次），失败插件回落旧版本缓存。
    let seed_root = storage.clone();
    let fallback =
        tokio::task::spawn_blocking(move || super::official_plugins::seed_once(&seed_root))
            .await
            .unwrap_or_default();
    for root in fallback {
        candidates.push((root, official.into(), "official", false, true));
    }
    let partition =
        json_file(&storage.join(format!("marketplaces/{official}/bundled-marketplace.json")))
            .await?;
    if let Some(entries) = partition["manifest"]["plugins"].as_array() {
        for entry in entries {
            if let (Some(path), Some(name)) = (entry["cachePath"].as_str(), entry["name"].as_str())
            {
                let root = resolve(&storage, path);
                let boundary = storage.join("cache").join(official).join(name);
                if root.starts_with(&boundary) && root != boundary {
                    candidates.push((root, official.into(), "official", false, true));
                }
            }
        }
    } else {
        for plugin in directories(&storage.join("cache").join(official)).await? {
            for version in directories(&plugin).await? {
                candidates.push((version, official.into(), "official", false, true));
            }
        }
    }
    let installed = json_file(&storage.join("installed_plugins.json")).await?;
    let mut records = vec![];
    if let Some(entries) = installed["plugins"].as_array() {
        records.extend(entries.iter().cloned());
    }
    if let Some(entries) = installed["plugins"].as_object() {
        for (id, value) in entries {
            for value in value
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![value.clone()])
            {
                if let Some((name, market)) = id.rsplit_once('@') {
                    let mut value = value;
                    value["name"] = name.into();
                    value["marketplace"] = market.into();
                    records.push(value);
                }
            }
        }
    }
    for record in records {
        if let (Some(name), Some(market)) =
            (record["name"].as_str(), record["marketplace"].as_str())
        {
            let root = record["installPath"]
                .as_str()
                .map(|p| resolve(&storage, p))
                .unwrap_or_else(|| {
                    storage
                        .join("cache")
                        .join(market)
                        .join(name)
                        .join(record["version"].as_str().unwrap_or("0.0.0"))
                });
            candidates.push((root, market.into(), "cache", false, false));
        }
    }
    let defaults: Vec<String> = serde_json::from_str(include_str!("plugin_defaults.json"))?;
    let mut seen = BTreeSet::new();
    let mut plugins = vec![];
    for (root, market, source, default, official_source) in candidates {
        super::tools::check_cancel(cancel)?;
        // TS loadPlugin：坏插件只出诊断并跳过，不能让整次发现失败。
        let Some(manifest) = load_manifest(&root, &mut diagnostics).await else {
            continue;
        };
        let name = manifest["name"].as_str().unwrap_or_default().to_owned();
        let id = format!("{name}@{market}");
        if official_source
            && strings(&config["plugins"]["suppressedBuiltins"]).contains(&id.as_str())
        {
            continue;
        }
        if !seen.insert(id.clone()) {
            diagnostics.push(diagnostic(
                "plugin_duplicate_id",
                format!("Duplicate plugin ignored: {id}"),
                Some(&id),
                "warning",
            ));
            continue;
        }
        // TS warnUnsupportedComponents。
        for key in ["channels", "lspServers", "outputStyles", "settings"] {
            if manifest.get(key).is_some() {
                diagnostics.push(diagnostic(
                    "plugin_unsupported_component",
                    format!("Plugin component is diagnostic-only in this ESCode runtime: {key}"),
                    Some(&id),
                    "warning",
                ));
            }
        }
        let enabled = config["plugins"]["enabledPlugins"][&id]
            .as_bool()
            .unwrap_or(default || defaults.contains(&id));
        plugins.push(Plugin {
            id,
            root,
            name,
            manifest,
            official: official_source,
            marketplace: market,
            source,
            enabled,
        });
    }
    Ok((plugins, diagnostics))
}

/// 协议诊断（TS `toPluginDiagnostic`：`path` 不出协议）。
fn diagnostic(code: &str, message: String, plugin_id: Option<&str>, severity: &str) -> Value {
    let mut value = serde_json::json!({ "code": code, "message": message, "severity": severity });
    if let Some(id) = plugin_id {
        value["pluginId"] = id.into();
    }
    value
}

/// TS `loadPlugin` + `findManifest` + `readManifest`：根不存在 / 没有 manifest / manifest 非法分别出诊断。
/// manifest 按 `.escode-plugin` → `.claude-plugin` → `.codex-plugin` 取**第一个存在的文件**（存在但非法时
/// 不再回退）；`name` trim 后必须匹配 `^[a-z0-9][a-z0-9._-]{0,127}$`，`version` 缺省为 `"0.0.0"`。
async fn load_manifest(root: &Path, diagnostics: &mut Vec<Value>) -> Option<Value> {
    if !tokio::fs::metadata(root).await.is_ok_and(|m| m.is_dir()) {
        diagnostics.push(diagnostic(
            "plugin_root_not_found",
            format!("Plugin root does not exist: {}", root.display()),
            None,
            "warning",
        ));
        return None;
    }
    let mut manifest_path = None;
    for dir in [".escode-plugin", ".claude-plugin", ".codex-plugin"] {
        let path = root.join(dir).join("plugin.json");
        if tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_file()) {
            manifest_path = Some(path);
            break;
        }
    }
    let Some(manifest_path) = manifest_path else {
        diagnostics.push(diagnostic(
            "plugin_manifest_not_found",
            format!("Plugin manifest not found: {}", root.display()),
            None,
            "error",
        ));
        return None;
    };
    let parsed = tokio::fs::read(&manifest_path)
        .await
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()))
        .and_then(|value| {
            if !value.is_object() {
                return Err("Manifest must be a JSON object".to_owned());
            }
            let name = value["name"].as_str().unwrap_or_default().trim().to_owned();
            let valid = name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
            if !valid {
                return Err(format!("Invalid plugin name: {name}"));
            }
            let mut value = value;
            value["name"] = name.into();
            if !value["version"].is_string() {
                value["version"] = "0.0.0".into();
            }
            Ok(value)
        });
    match parsed {
        Ok(manifest) => Some(manifest),
        Err(message) => {
            diagnostics.push(diagnostic(
                "plugin_manifest_invalid",
                message,
                None,
                "error",
            ));
            None
        }
    }
}
pub(super) async fn directories(root: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(dir) => dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut dirs = vec![];
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}
pub(super) async fn contained_file(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut current = root.to_owned();
    for part in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(part) = part {
            current.push(part);
        }
        if !tokio::fs::symlink_metadata(&current)
            .await
            .is_ok_and(|m| !m.file_type().is_symlink())
        {
            return false;
        }
    }
    true
}
