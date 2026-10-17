//! 插件命令来源（TS `commandsService.ts` 的插件根解析一族，`rust-u2-commands.md` §1）。
//!
//! 候选根三类，按序合并：CLI config 的 `plugins.dirs`（inline 市场）→ 官方缓存根
//! （`<pluginStorage>/cache/zcode-plugins-official/<plugin>/<version>/`）→
//! `installed_plugins.json` 里的安装根。每个候选根读一个插件清单（四种目录格式），
//! 清单里的 `commands` 字段给出命令子目录；缺省回 `<root>/commands`。
//!
//! 注意：**desktop 直接扫官方 cache，不经过 CLI resolve 的过滤**，所以被「卸载」的内置插件
//! （只写在 `plugins.suppressedBuiltins`）必须在这里显式跳过，否则仍会从 cache 贡献命令。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

const OFFICIAL_MARKETPLACE: &str = "zcode-plugins-official";
const INLINE_MARKETPLACE: &str = "inline";
const HOME_PREFIX: &str = "~/";
const MANIFEST_PATHS: [&str; 4] = [
    ".escode-plugin/plugin.json",
    // 旧 manifest 目录：Z.ai 官方 CDN 与既有第三方插件仍以 .zcode-plugin 发布。
    ".zcode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
];

/// TS `PluginConfigSummary`。
pub struct PluginConfig {
    pub dirs: Vec<String>,
    pub enabled: bool,
    pub enabled_plugins: BTreeMap<String, bool>,
    pub storage_dir: String,
    pub suppressed_builtins: Vec<String>,
}

pub struct ManifestSummary {
    pub name: String,
    /// `commands` 字段原值（`undefined` 与空列表语义不同：前者回退默认目录）。
    pub commands: Option<Value>,
}

pub struct RootCandidate {
    pub default_enabled: bool,
    pub marketplace: String,
    pub root_path: PathBuf,
}

pub struct RootDescriptor {
    pub plugin_enabled: bool,
    pub plugin_marketplace: String,
    pub plugin_name: String,
    pub root_path: PathBuf,
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn boolean_record(value: Option<&Value>) -> BTreeMap<String, bool> {
    let mut result = BTreeMap::new();
    if let Some(object) = value.and_then(Value::as_object) {
        for (key, entry) in object {
            if let Some(enabled) = entry.as_bool() {
                result.insert(key.clone(), enabled);
            }
        }
    }
    result
}

/// TS `readPluginConfigFromConfig`。
pub fn config_from(config: &Value) -> PluginConfig {
    let plugins = config.get("plugins").filter(|value| value.is_object());
    let storage_dir = config
        .get("storage")
        .and_then(|storage| storage.get("dir"))
        .and_then(Value::as_str)
        .filter(|dir| !dir.trim().is_empty())
        .unwrap_or(HOME_PREFIX_TRIM);
    PluginConfig {
        dirs: string_array(plugins.and_then(|plugins| plugins.get("dirs"))),
        enabled: plugins
            .and_then(|plugins| plugins.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        enabled_plugins: boolean_record(plugins.and_then(|plugins| plugins.get("enabledPlugins"))),
        storage_dir: storage_dir.to_owned(),
        suppressed_builtins: string_array(
            plugins.and_then(|plugins| plugins.get("suppressedBuiltins")),
        ),
    }
}

/// TS 缺省 `"~/.escode"`。
const HOME_PREFIX_TRIM: &str = "~/.escode";

fn is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
}

/// 词法归一：折叠 `.`，`..` 弹栈（不解析符号链接——`resolveInside` 的越界判定依赖这一点）。
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// TS `resolveConfigPath`：`~/` 前缀展开到 home；绝对路径原样；其余按**进程 cwd** 解析。
fn resolve_config_path(home: &Path, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix(HOME_PREFIX) {
        return normalize_lexical(&home.join(rest));
    }
    if is_absolute(path) {
        return normalize_lexical(Path::new(path));
    }
    // TS `resolve(expanded)` 以进程 cwd 为基准；Host 不读 `std::env`，走 core_api 的唯一出口。
    match escode_cli_core_api::env::current_dir() {
        Ok(cwd) => normalize_lexical(&cwd.join(path)),
        Err(_) => normalize_lexical(Path::new(path)),
    }
}

/// TS `resolveCliStorageRoot`：末段是 `cli` 就用它，否则补 `/cli`。
fn resolve_cli_storage_root(home: &Path, storage_dir: &str) -> PathBuf {
    let root = resolve_config_path(home, storage_dir);
    if root.file_name().and_then(|name| name.to_str()) == Some("cli") {
        root
    } else {
        root.join("cli")
    }
}

fn resolve_plugin_storage_root(home: &Path, storage_dir: &str) -> PathBuf {
    resolve_cli_storage_root(home, storage_dir).join("plugins")
}

/// TS `resolveInside`：只接受相对路径，且归一化后必须仍在根内（含根本身）。
fn resolve_inside(root: &Path, raw: &str) -> Option<PathBuf> {
    if is_absolute(raw) {
        return None;
    }
    let resolved = normalize_lexical(&root.join(raw));
    resolved.strip_prefix(root).ok().map(|_| resolved.clone())
}

/// TS `parsePathList`：字符串或字符串数组。
fn parse_path_list(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => vec![text.clone()],
        other => string_array(Some(other)),
    }
}

/// TS `scanOfficialPluginCacheRoots`：`<pluginStorage>/cache/<official>/<plugin>/<version>/`。
async fn scan_official_cache_roots(plugin_storage: &Path) -> Vec<PathBuf> {
    let cache_root = plugin_storage.join("cache").join(OFFICIAL_MARKETPLACE);
    let Ok(mut plugins) = tokio::fs::read_dir(&cache_root).await else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    while let Ok(Some(plugin)) = plugins.next_entry().await {
        let plugin_dir = plugin.path();
        let Ok(mut versions) = tokio::fs::read_dir(&plugin_dir).await else {
            continue;
        };
        while let Ok(Some(version)) = versions.next_entry().await {
            let root = version.path();
            if let Ok(meta) = tokio::fs::symlink_metadata(&root).await
                && meta.is_dir()
            {
                roots.push(root);
            }
        }
    }
    // TS `localeCompare`：Host 侧统一用 ICU 排序器（`collation::compare_names`）外，这里只做稳定排序。
    roots.sort();
    roots
}

/// TS `findPluginManifestPath`：四种目录格式按序取第一个存在的。
async fn find_manifest_path(root: &Path) -> Option<PathBuf> {
    for relative in MANIFEST_PATHS {
        let candidate = root.join(relative);
        if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return Some(candidate);
        }
    }
    None
}

/// TS `readPluginManifest`：`name` 必须匹配 `^[a-z0-9][a-z0-9._-]{0,127}$`（trim 后）。
async fn read_manifest(root: &Path) -> Option<ManifestSummary> {
    let manifest_path = find_manifest_path(root).await?;
    let raw = tokio::fs::read_to_string(manifest_path).await.ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let object = parsed.as_object()?;
    let name = object.get("name").and_then(Value::as_str)?.trim();
    if !is_plugin_name(name) {
        return None;
    }
    Some(ManifestSummary {
        name: name.to_owned(),
        commands: object.get("commands").cloned(),
    })
}

fn is_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) || name.len() > 128 {
        return false;
    }
    bytes[1..].iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
    })
}

/// TS `resolvePluginCommandRoots`：清单 `commands` 指向的子目录；字段缺省时回退 `<root>/commands`。
async fn resolve_command_roots(manifest: &ManifestSummary, root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(commands) = &manifest.commands {
        for raw in parse_path_list(commands) {
            if let Some(path) = resolve_inside(root, &raw) {
                roots.push(path);
            }
        }
    }
    if roots.is_empty() && manifest.commands.is_none() {
        let default_root = root.join("commands");
        if tokio::fs::try_exists(&default_root).await.unwrap_or(false) {
            roots.push(default_root);
        }
    }
    roots
}

/// TS `readInstalledPluginRoots`：`<pluginStorage>/installed_plugins.json`。
async fn read_installed_roots(plugin_storage: &Path) -> Vec<RootCandidate> {
    let Ok(raw) = tokio::fs::read_to_string(plugin_storage.join("installed_plugins.json")).await
    else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(plugins) = parsed.get("plugins").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for item in plugins {
        let Some(object) = item.as_object() else {
            continue;
        };
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("")
                .to_owned()
        };
        let (id, marketplace, install_path) =
            (text("id"), text("marketplace"), text("installPath"));
        if id.is_empty() || marketplace.is_empty() || !is_absolute(&install_path) {
            continue;
        }
        roots.push(RootCandidate {
            default_enabled: false,
            marketplace,
            root_path: PathBuf::from(install_path),
        });
    }
    roots
}

/// TS `resolvePluginCommandRootDescriptors`：候选根 → 清单 → 启停判定 → 命令子目录。
pub async fn resolve_root_descriptors(home: &Path, config: &Value) -> Vec<RootDescriptor> {
    let config = config_from(config);
    if !config.enabled {
        return Vec::new();
    }
    let plugin_storage = resolve_plugin_storage_root(home, &config.storage_dir);
    let mut candidates: Vec<RootCandidate> = config
        .dirs
        .iter()
        .map(|dir| RootCandidate {
            default_enabled: true,
            marketplace: INLINE_MARKETPLACE.to_owned(),
            root_path: resolve_config_path(home, dir),
        })
        .collect();
    for root_path in scan_official_cache_roots(&plugin_storage).await {
        candidates.push(RootCandidate {
            default_enabled: false,
            marketplace: OFFICIAL_MARKETPLACE.to_owned(),
            root_path,
        });
    }
    candidates.extend(read_installed_roots(&plugin_storage).await);

    let mut descriptors = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for candidate in candidates {
        let Some(manifest) = read_manifest(&candidate.root_path).await else {
            continue;
        };
        let plugin_id = format!("{}@{}", manifest.name, candidate.marketplace);
        // 被「卸载」的内置插件只写在 config 里；desktop 直接扫 cache 时必须同样跳过。
        if candidate.marketplace == OFFICIAL_MARKETPLACE
            && config.suppressed_builtins.contains(&plugin_id)
        {
            continue;
        }
        if !seen.insert(plugin_id.clone()) {
            continue;
        }
        let default_enabled =
            candidate.default_enabled || crate::official_plugins::is_default_enabled(&plugin_id);
        let enabled = config
            .enabled_plugins
            .get(&plugin_id)
            .copied()
            .unwrap_or(default_enabled);
        if !enabled {
            continue;
        }
        for root_path in resolve_command_roots(&manifest, &candidate.root_path).await {
            descriptors.push(RootDescriptor {
                plugin_enabled: enabled,
                plugin_marketplace: candidate.marketplace.clone(),
                plugin_name: manifest.name.clone(),
                root_path,
            });
        }
    }
    descriptors
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod tests;
