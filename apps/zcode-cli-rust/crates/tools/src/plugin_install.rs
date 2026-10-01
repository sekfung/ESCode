//! `plugins/install`（docs/specs/rust-plugin-marketplace-write.md W1b：本地源）：对齐 TS `installPlugin` →
//! `installZCodeMarketplacePlugin` → adapters `installMarketplacePlugin` / `cacheMarketplacePlugin`。
//! 远端源（zip / git / github）在 W2 / W3 实现之前按 TS「recognized but not supported」同一条诊断返回。

use super::plugin_marketplace::{self as market, OFFICIAL_MARKETPLACE, sanitize};
use super::plugin_uninstall::{read_installed_sync, storage_lock};
use super::{atomic_dir, config_file};
use super::{extension_config as config, extension_plugins as plugins, plugin_list};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub(super) async fn install(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let name = plugin_list::non_empty(params, "pluginName")?.to_owned();
    let marketplace = plugin_list::non_empty(params, "marketplace")?.to_owned();
    plugin_list::scope_of(params)?;
    if params["dryRun"] == true {
        bail!("plugins/install dryRun (validation) is not supported by the Rust runtime yet");
    }
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    market::ensure_default_marketplaces(&storage)?;
    let plugin_id = format!("{name}@{marketplace}");
    let user_path = config::home()
        .join(".zcode")
        .join("cli")
        .join("config.json");

    // 被抑制的内置官方插件（filesystem / sea 源）只是目录指针：复用 restore，不写 installed record。
    let bundled_source = ordered_entry(&storage, &marketplace, &name).and_then(|entry| {
        entry
            .get("source")
            .and_then(Json::as_str)
            .map(str::to_owned)
    });
    let suppressed =
        config::strings(&config["plugins"]["suppressedBuiltins"]).contains(&plugin_id.as_str());
    if marketplace == OFFICIAL_MARKETPLACE
        && suppressed
        && matches!(bundled_source.as_deref(), Some("filesystem" | "sea"))
    {
        if plugin_id == "computer-use@zcode-plugins-official"
            && !super::plugin_overview::cua_feature_enabled()
        {
            bail!("computer-use built-in plugin requires ZCODE_CUA_PRODUCT_HELPER to be enabled");
        }
        return restore_bundled(&cwd, &storage, &plugin_id, &user_path, cancel).await;
    }

    // TS ensureMarketplaceManifestAvailable：本地没有目录 manifest 但有已知记录时先拉取（失败归入安装诊断）。
    if let Err(error) = super::plugin_market_write::ensure_manifest(&storage, &marketplace).await {
        return Ok(json!({
            "dependencyClosure": [],
            "installedPlugins": [],
            "diagnostics": [install_diagnostic(&error, &plugin_id)],
        }));
    }
    let worker_storage = storage.clone();
    let (worker_market, worker_name) = (marketplace.clone(), name.clone());
    let outcome = tokio::task::spawn_blocking(move || {
        install_closure(&worker_storage, &worker_market, &worker_name)
    })
    .await
    .map_err(|_| anyhow!("Plugin install worker panicked"))?;
    let (closure, records) = match outcome {
        Ok(done) => done,
        Err(error) => {
            return Ok(json!({
                "dependencyClosure": [],
                "installedPlugins": [],
                "diagnostics": [install_diagnostic(&error, &plugin_id)],
            }));
        }
    };
    let ids: Vec<String> = records
        .iter()
        .filter_map(|r| r.get("id").and_then(Json::as_str).map(str::to_owned))
        .collect();
    if marketplace == OFFICIAL_MARKETPLACE {
        // 同名 CDN 插件重装：清掉历史内置 suppression，否则运行时仍判为被抑制。
        for id in &ids {
            config_file::patch_file(&user_path, |file| {
                config_file::remove_suppressed_builtin(file, id)
            })
            .await?;
        }
    }
    // 安装即默认启用：只给用户配置里尚未显式声明的 id 写 true（停用后重装不被覆盖）。
    let mut newly_enabled: Vec<String> = vec![];
    config_file::patch_file(&user_path, |file| {
        newly_enabled = config_file::enable_by_default(file, &ids);
        !newly_enabled.is_empty()
    })
    .await?;
    let installed: Vec<Value> = records
        .iter()
        .map(|record| {
            let id = record.get("id").and_then(Json::as_str).unwrap_or_default();
            let enabled = newly_enabled.iter().any(|n| n == id)
                || config["plugins"]["enabledPlugins"][id]
                    .as_bool()
                    .unwrap_or(false);
            summary(
                &serde_json::from_str(&record.compact()).unwrap_or_default(),
                enabled,
            )
        })
        .collect();
    Ok(json!({ "dependencyClosure": closure, "installedPlugins": installed, "diagnostics": [] }))
}

/// TS `toInstalledPluginSummary(toInstalledPluginData(record, enabled))`（未带加载元数据）。
fn summary(record: &Value, enabled: bool) -> Value {
    let mut out = json!({
        "id": record["id"],
        "name": record["name"],
        "marketplace": record["marketplace"],
        "enabled": enabled,
        "scope": record["scope"],
    });
    for key in ["version", "installPath", "installedAt"] {
        if record[key].as_str().is_some_and(|s| !s.is_empty()) {
            out[key] = record[key].clone();
        }
    }
    out
}

async fn restore_bundled(
    cwd: &Path,
    storage: &Path,
    plugin_id: &str,
    user_path: &Path,
    cancel: &CancellationToken,
) -> Result<Value> {
    config_file::patch_file(user_path, |file| {
        config_file::remove_suppressed_builtin(file, plugin_id)
    })
    .await?;
    let seed_root = storage.to_owned();
    tokio::task::spawn_blocking(move || super::official_plugins::reseed(&seed_root))
        .await
        .map_err(|_| anyhow!("Official plugin reseed panicked"))?;
    let fresh = config::load(cwd).await?;
    let discovered = plugins::all(cwd, &fresh, cancel).await?;
    let Some(restored) = discovered.iter().find(|p| p.id == plugin_id) else {
        return Ok(json!({
            "dependencyClosure": [],
            "installedPlugins": [],
            "diagnostics": [{
                "code": "plugin_not_found",
                "message": format!("Bundled plugin could not be restored: {plugin_id}"),
                "severity": "error",
                "pluginId": plugin_id,
            }],
        }));
    };
    let data_root = config::storage(&fresh).join("data");
    let info = plugin_list::info(restored, cwd, &fresh, &data_root).await?;
    let mut item = summary(
        &json!({
            "id": restored.id,
            "name": restored.name,
            "marketplace": restored.marketplace,
            "version": restored.manifest["version"],
            "installPath": restored.root.to_string_lossy(),
            "installedAt": iso_now(),
            "scope": "user",
        }),
        restored.enabled,
    );
    if let Some(description) = restored.manifest["description"]
        .as_str()
        .filter(|d| !d.is_empty())
    {
        item["description"] = description.into();
    }
    item["componentTypes"] = super::plugin_overview::component_types_from_info(&info);
    Ok(json!({ "dependencyClosure": [plugin_id], "installedPlugins": [item], "diagnostics": [] }))
}

fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// TS `toMarketplaceInstallDiagnostic`：按错误文案归类。
fn install_diagnostic(error: &anyhow::Error, plugin_id: &str) -> Value {
    let message = error.to_string();
    let message = message.as_str();
    let source_code = error
        .downcast_ref::<super::plugin_git::SourceError>()
        .map(|e| e.code);
    let code = if let Some(code) = source_code {
        code
    } else if message.starts_with("Plugin not found:") {
        "plugin_not_found"
    } else if message.contains("Cross-marketplace dependency") {
        "plugin_dependency_cross_marketplace"
    } else if message.contains("dependency cycle") {
        "plugin_dependency_cycle"
    } else if message.contains("Dependency not found")
        || message.contains("Marketplace not found for dependency")
    {
        "plugin_dependency_missing"
    } else if message.contains("source is recognized but not supported") {
        "plugin_marketplace_source_unsupported"
    } else {
        "plugin_marketplace_invalid"
    };
    json!({ "code": code, "message": message, "severity": "error", "pluginId": plugin_id })
}

// ---- 阻塞线程里的存储工作 ----

/// 市场 manifest 原文（保序）。TS 用解析后的条目，但合成 plugin.json 与 installed record 的 `source`
/// 都要按条目原样的 key 顺序输出。
fn ordered_manifest(storage: &Path, marketplace: &str) -> Option<Json> {
    let path = storage
        .join("marketplaces")
        .join(sanitize(marketplace))
        .join("marketplace.json");
    let text = std::fs::read_to_string(atomic_dir::recover(&path)).ok()?;
    Json::parse(&text).filter(Json::is_object)
}

/// 条目原文：`plugins` 为数组时按 trim 后的 name 匹配；为对象时 key 即 name（补上 `name`）。
fn ordered_entry(storage: &Path, marketplace: &str, name: &str) -> Option<Json> {
    let manifest = ordered_manifest(storage, marketplace)?;
    match manifest.get("plugins")? {
        Json::Array(items) => items
            .iter()
            .find(|item| item.get("name").and_then(Json::as_str).map(str::trim) == Some(name))
            .cloned(),
        Json::Object(entries) => entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(key, value)| {
                let mut entry = Json::object();
                entry.set("name", Json::str(key));
                if let Json::Object(fields) = value {
                    for (k, v) in fields {
                        entry.set(k, v.clone());
                    }
                }
                entry
            }),
        _ => None,
    }
}

/// TS `normalizeDependencyRef`：installed record 的 `dependencies` 记录的就是这一份（未限定市场）。
fn normalized_dependencies(entry: &Json) -> Vec<String> {
    let Some(items) = entry.get("dependencies").and_then(Json::as_array) else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| match item {
            Json::String(text) => {
                // `name@^1.2` 的版本约束后缀：去掉最后一段以 `@^` 开头的部分。
                let trimmed = match text.rfind("@^") {
                    Some(at) if !text[at + 2..].contains('@') => &text[..at],
                    _ => text.as_str(),
                };
                Some(trimmed.to_owned())
            }
            Json::Object(_) => {
                let name = item.get("name").and_then(Json::as_str)?.trim().to_owned();
                if name.is_empty() {
                    return None;
                }
                let market = item
                    .get("marketplace")
                    .and_then(Json::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                Some(if market.is_empty() {
                    name
                } else {
                    format!("{name}@{market}")
                })
            }
            _ => None,
        })
        .collect()
}

/// TS `qualifyDependency`：未带市场的依赖属于条目所在市场。
fn dependencies(entry: &Json, marketplace: &str) -> Vec<String> {
    normalized_dependencies(entry)
        .into_iter()
        .map(|dep| {
            if dep.contains('@') {
                dep
            } else {
                format!("{dep}@{marketplace}")
            }
        })
        .collect()
}

fn split_id(id: &str) -> Result<(&str, &str)> {
    match id.rfind('@') {
        Some(at) if at > 0 && at < id.len() - 1 => Ok((&id[..at], &id[at + 1..])),
        _ => bail!("Plugin id must use <name>@<marketplace>: {id}"),
    }
}

/// TS `resolveDependencyClosure`：深度优先、后序（依赖先于依赖者）；跨市场需根市场白名单。
fn closure(storage: &Path, marketplace: &str, name: &str) -> Result<Vec<String>> {
    let allow: Vec<String> = ordered_manifest(storage, marketplace)
        .and_then(|m| m.get("allowCrossMarketplaceDependenciesOn").cloned())
        .and_then(|v| {
            v.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        })
        .unwrap_or_default();
    let root = format!("{name}@{marketplace}");
    let mut out = vec![];
    let mut visiting: Vec<String> = vec![];
    fn walk(
        storage: &Path,
        root_market: &str,
        allow: &[String],
        id: &str,
        required_by: &str,
        visiting: &mut Vec<String>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        let (name, market) = split_id(id)?;
        if market != root_market && !allow.iter().any(|a| a == market) {
            bail!("Cross-marketplace dependency is blocked: {id} required by {required_by}");
        }
        if visiting.iter().any(|v| v == id) {
            bail!("Plugin dependency cycle: {} -> {id}", visiting.join(" -> "));
        }
        if out.iter().any(|v| v == id) {
            return Ok(());
        }
        if market::manifest(storage, market).is_none() {
            bail!("Marketplace not found for dependency: {market}");
        }
        let entry = ordered_entry(storage, market, name)
            .ok_or_else(|| anyhow!("Dependency not found: {id} required by {required_by}"))?;
        visiting.push(id.to_owned());
        for dependency in dependencies(&entry, market) {
            walk(storage, root_market, allow, &dependency, id, visiting, out)?;
        }
        visiting.pop();
        out.push(id.to_owned());
        Ok(())
    }
    walk(
        storage,
        marketplace,
        &allow,
        &root,
        &root,
        &mut visiting,
        &mut out,
    )?;
    Ok(out)
}

/// TS `findPluginManifestPath`。
fn manifest_path(root: &Path) -> Option<PathBuf> {
    [".zcode-plugin", ".claude-plugin", ".codex-plugin"]
        .iter()
        .map(|dir| root.join(dir).join("plugin.json"))
        .find(|path| path.is_file())
}

/// TS `resolveInside`：相对路径解析后必须仍在 base 内。
fn resolve_inside(base: &Path, relative: &str) -> Option<PathBuf> {
    let joined = super::lexical_path::normalize(&base.join(relative));
    joined
        .starts_with(super::lexical_path::normalize(base))
        .then_some(joined)
}

/// TS `resolvePluginSourceRoot`（本地源部分）。
fn source_root(storage: &Path, marketplace: &str, entry: &Json) -> Result<PathBuf> {
    let name = entry.get("name").and_then(Json::as_str).unwrap_or_default();
    let id = format!("{name}@{marketplace}");
    let marketplace_dir = storage.join("marketplaces").join(sanitize(marketplace));
    let plugin_root = ordered_manifest(storage, marketplace)
        .and_then(|m| m.get("metadata").cloned())
        .and_then(|meta| {
            meta.get("pluginRoot")
                .and_then(Json::as_str)
                .map(str::to_owned)
        });
    let base = plugin_root
        .and_then(|root| resolve_inside(&marketplace_dir, &root))
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| marketplace_dir.clone());
    match entry.get("source") {
        Some(Json::String(kind)) if kind == "filesystem" || kind == "sea" => {
            if let Some(cache) = entry
                .get("cachePath")
                .and_then(Json::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
            {
                return Ok(cache);
            }
            let version = entry
                .get("version")
                .and_then(Json::as_str)
                .unwrap_or("0.0.0");
            let computed = cache_dir(storage, marketplace, name, version);
            if computed.is_dir() {
                return Ok(computed);
            }
            bail!("Bundled plugin cache directory missing: {id}")
        }
        Some(Json::String(path)) => {
            let relative = path.strip_prefix("./").unwrap_or(path);
            if let Some(local) = resolve_inside(&base, relative).filter(|p| p.is_dir()) {
                return Ok(local);
            }
            let absolute = PathBuf::from(path);
            if absolute.is_absolute() && absolute.is_dir() {
                return Ok(absolute);
            }
            bail!("Unsupported or missing plugin source: {path}")
        }
        Some(source @ Json::Object(_)) => {
            let kind = source
                .get("source")
                .and_then(Json::as_str)
                .unwrap_or_default();
            match kind {
                "directory" => {
                    let path = source
                        .get("path")
                        .and_then(Json::as_str)
                        .filter(|p| !p.trim().is_empty())
                        .ok_or_else(|| {
                            anyhow!("Plugin directory path source requires a non-empty path")
                        })?;
                    let path = super::lexical_path::normalize(Path::new(path));
                    if path.is_dir() {
                        return Ok(path);
                    }
                    bail!("Plugin source directory does not exist: {}", path.display())
                }
                // 仓库类与 zip 源在 source_root 之前已分流；剩下的 url:<其它 type> 与 npm / pip 同 TS 不支持。
                "url" | "npm" | "pip" => {
                    let label = if kind == "url" {
                        match source.get("type").and_then(Json::as_str) {
                            Some(t) if !t.is_empty() => format!("url:{t}"),
                            _ => "url".to_owned(),
                        }
                    } else {
                        kind.to_owned()
                    };
                    bail!("Plugin source is recognized but not supported in this runtime: {label}")
                }
                _ => bail!(
                    "Plugin source is invalid or unsupported for {id}: {}",
                    if kind.is_empty() {
                        "missing kind"
                    } else {
                        kind
                    }
                ),
            }
        }
        _ => {
            let local = base.join(name);
            if local.is_dir() {
                return Ok(local);
            }
            bail!("Plugin source is not supported for {id}")
        }
    }
}

/// 仓库类源（TS `resolvePluginSourceRoot` 的 github / git / url(git) / git-subdir 分支）；字段校验文案同 TS。
fn repository_source(entry: &Json) -> Result<Option<super::plugin_git::RepoSource>> {
    let Some(source) = entry.get("source").filter(|s| s.is_object()) else {
        return Ok(None);
    };
    let text = |key: &str| source.get(key).and_then(Json::as_str).map(str::to_owned);
    let required = |key: &str, label: &str| {
        text(key)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow!("Plugin {label} source requires a non-empty {key}"))
    };
    let kind = text("source").unwrap_or_default();
    // TS readPluginSourceIdentityPin：仓库源没有 zip sha256，取 sha，其次旧写法 commit。
    let sha = text("sha").or_else(|| text("commit"));
    let repo = match kind.as_str() {
        "github" => {
            let repo = required("repo", "GitHub repo")?;
            super::plugin_git::RepoSource {
                url: format!("https://github.com/{repo}.git"),
                path: text("path"),
                r#ref: text("ref"),
                sha,
            }
        }
        "git" => super::plugin_git::RepoSource {
            url: required("url", "Git URL")?,
            path: text("path"),
            r#ref: text("ref"),
            sha,
        },
        "url" => {
            let url = required("url", "URL")?;
            match text("type").unwrap_or_default().as_str() {
                "" | "git" => super::plugin_git::RepoSource {
                    url,
                    path: text("path"),
                    r#ref: text("ref"),
                    sha,
                },
                _ => return Ok(None),
            }
        }
        "git-subdir" => {
            let path = required("path", "git-subdir path")?;
            let url = required("url", "git-subdir URL")?;
            // TS normalizeGitUrl：owner/repo 简写补成 GitHub HTTPS。
            let is_short = url.split('/').count() == 2
                && !url.contains(':')
                && url.split('/').all(|part| !part.is_empty());
            super::plugin_git::RepoSource {
                url: if is_short {
                    format!("https://github.com/{url}.git")
                } else {
                    url
                },
                path: Some(path),
                r#ref: text("ref"),
                sha,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(repo))
}

/// `{source: "url", type: "zip"}`（TS `isZipPluginUrlSource` 的入口判定；字段校验在 plugin_zip）。
fn is_zip_source(entry: &Json) -> bool {
    let Some(source) = entry.get("source").filter(|s| s.is_object()) else {
        return false;
    };
    source.get("source").and_then(Json::as_str) == Some("url")
        && source.get("type").and_then(Json::as_str) == Some("zip")
}

/// TS `assertZipPluginInstallRoot` + `readPluginManifestFromRoot`：多顶层 zip 未指定 path 时会回退到解压根，
/// 必须确认它能形成合法插件且名字与目录条目一致，否则不能写安装记录。
fn assert_zip_root(root: &Path, entry: &Json, marketplace: &str) -> Result<()> {
    let entry_name = entry.get("name").and_then(Json::as_str).unwrap_or_default();
    let id = format!("{entry_name}@{marketplace}");
    let name = match manifest_path(root) {
        Some(path) => {
            let parsed: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            if !parsed.is_object() {
                bail!("Plugin manifest must be a JSON object");
            }
            let name = parsed["name"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_owned();
            let valid = name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
            if !valid {
                bail!("Invalid plugin name: {name}");
            }
            name
        }
        None if entry.get("strict") == Some(&Json::Bool(false)) => entry_name.trim().to_owned(),
        None => bail!("Plugin manifest not found: {id}"),
    };
    if name != entry_name {
        bail!("Plugin manifest name '{name}' does not match marketplace entry '{entry_name}'");
    }
    Ok(())
}

/// TS `getPluginCacheDir`。
fn cache_dir(storage: &Path, marketplace: &str, name: &str, version: &str) -> PathBuf {
    storage
        .join("cache")
        .join(sanitize(marketplace))
        .join(sanitize(name))
        .join(sanitize(version))
}

/// TS `resolveInstalledPluginVersion`：源根 plugin.json 的非空 version > 条目 version > 0.0.0。
fn installed_version(root: &Path, entry: &Json) -> String {
    let from_manifest = manifest_path(root)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value["version"].as_str().map(str::to_owned))
        .filter(|v| !v.trim().is_empty());
    from_manifest.unwrap_or_else(|| {
        entry
            .get("version")
            .and_then(Json::as_str)
            .unwrap_or("0.0.0")
            .to_owned()
    })
}

/// TS `ensureMarketplaceEntryManifest`：目标没有 plugin.json 且条目 `strict: false` 时合成
/// `.claude-plugin/plugin.json`（剔除来源 / 商店展示字段，补 name / version）。
fn ensure_entry_manifest(entry: &Json, target: &Path) -> Result<()> {
    if manifest_path(target).is_some() || entry.get("strict") != Some(&Json::Bool(false)) {
        return Ok(());
    }
    let mut manifest = entry.clone();
    for key in [
        "source",
        "category",
        "tags",
        "strict",
        "displayName",
        "displayName_i18n",
        "description_i18n",
        "icon",
        "privacyPolicy",
        "termsOfService",
        "heroImage",
        "examplePrompts",
        "examplePrompts_i18n",
        "requiresPaidPlan",
    ] {
        manifest.remove(key);
    }
    let name = entry
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    manifest.set("name", Json::str(name));
    let version = entry
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or("0.0.0")
        .to_owned();
    manifest.set("version", Json::str(version));
    let dir = target.join(".claude-plugin");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("plugin.json"), format!("{}\n", manifest.pretty()))?;
    Ok(())
}

/// TS `installMarketplacePlugin`：解析闭包 → 逐个激活缓存 → 写 installed_plugins.json；
/// 任一步失败按逆序回滚已激活目录。
fn install_closure(
    storage: &Path,
    marketplace: &str,
    name: &str,
) -> Result<(Vec<String>, Vec<Json>)> {
    let ids = closure(storage, marketplace, name)?;
    let mut state = read_installed_sync(storage);
    let authority = storage.join("installed_plugins.json");
    let mut installed = vec![];
    let mut activations = vec![];
    let now = iso_now();
    let result = (|| -> Result<()> {
        for id in &ids {
            let (plugin_name, plugin_market) = split_id(id)?;
            let entry = ordered_entry(storage, plugin_market, plugin_name)
                .ok_or_else(|| anyhow!("Plugin not found: {id}"))?;
            // zip 源（W2）：下载解压到临时目录；激活后（无论成败）清理。
            // 远端源（W2 zip / W3 仓库）物化到临时目录；激活后（无论成败）清理。
            let zip = if is_zip_source(&entry) {
                let source = entry.get("source").cloned().unwrap_or(Json::Null);
                let root = tokio::runtime::Handle::current()
                    .block_on(super::plugin_zip::resolve(&source))?;
                if let Err(error) = assert_zip_root(&root.path, &entry, plugin_market) {
                    root.cleanup();
                    return Err(error);
                }
                Some((root.path.clone(), root.temp_dir()))
            } else if let Some(repo) = repository_source(&entry)? {
                let root =
                    tokio::runtime::Handle::current().block_on(super::plugin_git::resolve(repo))?;
                Some((root.path.clone(), root.temp_dir()))
            } else {
                None
            };
            let source = match &zip {
                Some((path, _)) => path.clone(),
                None => source_root(storage, plugin_market, &entry)?,
            };
            let version = installed_version(&source, &entry);
            let target = cache_dir(storage, plugin_market, plugin_name, &version);
            let same =
                super::lexical_path::normalize(&source) == super::lexical_path::normalize(&target);
            let mut transaction = None;
            let activated = if same {
                ensure_entry_manifest(&entry, &target).map(|_| None)
            } else {
                atomic_dir::activate(Some(&source), &target, &authority, |staged| {
                    ensure_entry_manifest(&entry, staged)
                })
                .map(Some)
            };
            // 缓存已复制（或失败）后临时目录清理失败不阻断安装记录落盘。
            if let Some((_, temp)) = &zip {
                let _ = std::fs::remove_dir_all(temp);
            }
            if let Some(activation) = activated? {
                transaction = Some(activation.transaction_id.clone());
                activations.push(activation);
            }
            let mut record = Json::object();
            record.set("id", Json::str(id.clone()));
            record.set("name", Json::str(plugin_name));
            record.set("marketplace", Json::str(plugin_market));
            record.set("version", Json::str(version.clone()));
            record.set("installPath", Json::str(target.to_string_lossy()));
            record.set("installedAt", Json::str(now.clone()));
            record.set("updatedAt", Json::str(now.clone()));
            record.set("scope", Json::str("user"));
            let deps = normalized_dependencies(&entry);
            if entry
                .get("dependencies")
                .is_some_and(|d| d.as_array().is_some())
            {
                record.set(
                    "dependencies",
                    Json::Array(deps.into_iter().map(Json::String).collect()),
                );
            }
            if let Some(source) = entry.get("source") {
                record.set("source", source.clone());
            }
            if let Some(transaction) = transaction {
                record.set("cacheTransactionId", Json::str(transaction));
            }
            // 已有记录：原地覆盖（去掉旧 cacheTransactionId），保留首次 installedAt。
            match state
                .iter_mut()
                .find(|existing| existing.get("id").and_then(Json::as_str) == Some(id.as_str()))
            {
                Some(existing) => {
                    let first = existing
                        .get("installedAt")
                        .cloned()
                        .unwrap_or_else(|| Json::str(now.clone()));
                    existing.remove("cacheTransactionId");
                    if let Json::Object(fields) = &record {
                        for (key, value) in fields {
                            existing.set(key, value.clone());
                        }
                    }
                    existing.set("installedAt", first);
                    installed.push(existing.clone());
                }
                None => {
                    state.push(record.clone());
                    installed.push(record);
                }
            }
        }
        let mut file = Json::object();
        file.set("version", Json::Number(1.into()));
        file.set("plugins", Json::Array(state.clone()));
        std::fs::create_dir_all(storage)?;
        let temp = storage.join(format!(
            ".installed_plugins.json.stage-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&temp, format!("{}\n", file.pretty()))?;
        std::fs::rename(&temp, &authority).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_error = None;
        for activation in activations.into_iter().rev() {
            if let Err(e) = activation.rollback() {
                rollback_error.get_or_insert(e);
            }
        }
        return Err(match rollback_error {
            Some(rollback) => error.context(format!("cleanup also failed: {rollback}")),
            None => error,
        });
    }
    for activation in activations {
        activation.finalize();
    }
    Ok((ids, installed))
}
