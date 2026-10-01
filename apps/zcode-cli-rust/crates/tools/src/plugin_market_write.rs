//! 插件市场写面（docs/specs/rust-plugin-marketplace-write.md W4）：`plugins/marketplace/add|remove|update` 与安装时的
//! 按需拉取。对齐 TS adapters `addMarketplace` / `updateMarketplace` / `removeMarketplace` 与 bootstrap
//! `addZCodePluginMarketplace` / `updateZCodePluginMarketplace`。

use super::plugin_marketplace::{OFFICIAL_MARKETPLACE, sanitize};
use super::{atomic_dir, plugin_git};
use crate::domain::json_order::Json;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const JSON_MAX_BYTES: usize = 10 * 1024 * 1024;
const JSON_MAX_REDIRECTS: usize = 5;
const JSON_TIMEOUT: Duration = Duration::from_millis(180_000);

/// TS `UnsupportedMarketplaceSourceError` / `UnsupportedPluginSourceError` 的文案前缀（诊断归类用）。
const UNSUPPORTED: &str = "source is recognized but not supported in this runtime";

fn iso_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn known_path(storage: &Path) -> PathBuf {
    storage.join("known_marketplaces.json")
}

fn valid_known(record: &Json) -> bool {
    record.is_object()
        && matches!(record.get("id"), Some(Json::String(_)))
        && matches!(record.get("name"), Some(Json::String(_)))
        && matches!(record.get("pluginCount"), Some(Json::Number(_)))
        && record.get("source").is_some_and(Json::is_object)
}

/// TS `loadKnownMarketplacesSync`（保序）：`marketplaces` 为数组或对象，只保留合法记录。
pub(super) fn known(storage: &Path) -> Vec<Json> {
    let path = atomic_dir::recover(&known_path(storage));
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| Json::parse(&text));
    let records = match parsed.as_ref().and_then(|p| p.get("marketplaces")) {
        Some(Json::Array(items)) => items.clone(),
        Some(Json::Object(entries)) => entries.iter().map(|(_, v)| v.clone()).collect(),
        _ => vec![],
    };
    records.into_iter().filter(valid_known).collect()
}

/// TS `writeKnownMarketplaces`（`writeJsonFile` → 原子替换）。
fn write_known(storage: &Path, records: Vec<Json>) -> Result<()> {
    let mut file = Json::object();
    file.set("version", Json::Number(1.into()));
    file.set("marketplaces", Json::Array(records));
    std::fs::create_dir_all(storage)?;
    let target = known_path(storage);
    let temp = storage.join(format!(
        ".known_marketplaces.json.stage-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&temp, format!("{}\n", file.pretty()))?;
    std::fs::rename(&temp, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;
    Ok(())
}

fn id_of(record: &Json) -> &str {
    record.get("id").and_then(Json::as_str).unwrap_or_default()
}

// ---- 源输入解析（TS parseMarketplaceSourceInput） ----

fn split_ref(input: &str) -> (String, Option<String>) {
    match input.rfind('#') {
        Some(at) => (input[..at].to_owned(), Some(input[at + 1..].to_owned())),
        None => (input.to_owned(), None),
    }
}

fn source(kind: &str, fields: &[(&str, Option<String>)]) -> Json {
    let mut out = Json::object();
    out.set("source", Json::str(kind));
    for (key, value) in fields {
        if let Some(value) = value {
            out.set(key, Json::str(value.clone()));
        }
    }
    out
}

/// TS `parseMarketplaceSourceInput`：URL / Git SSH / 本地路径（相对路径按工作区解析）/ GitHub 简写。
pub(super) fn parse_source_input(input: &str, cwd: &Path) -> Result<Json> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("Marketplace source is empty");
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let (url, r#ref) = split_ref(trimmed);
        if url.ends_with(".git") || url.contains("/_git/") {
            return Ok(source("git", &[("url", Some(url)), ("ref", r#ref)]));
        }
        if let Ok(parsed) = url::Url::parse(&url)
            && matches!(parsed.host_str(), Some("github.com" | "www.github.com"))
        {
            let segments: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();
            if segments.len() >= 2 && !segments[1].trim_end_matches(".git").is_empty() {
                let git_url = if url.ends_with(".git") {
                    url
                } else {
                    format!("{url}.git")
                };
                return Ok(source("git", &[("url", Some(git_url)), ("ref", r#ref)]));
            }
        }
        return Ok(source("url", &[("url", Some(url))]));
    }
    let ssh = trimmed.split_once('@').is_some_and(|(user, rest)| {
        !user.is_empty()
            && user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            && rest
                .split_once(':')
                .is_some_and(|(host, path)| !host.is_empty() && !path.is_empty())
    });
    if ssh {
        let (url, r#ref) = split_ref(trimmed);
        return Ok(source("git", &[("url", Some(url)), ("ref", r#ref)]));
    }
    let bytes = trimmed.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\');
    if trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.starts_with('/')
        || trimmed.starts_with('~')
        || drive
    {
        let resolved = match trimmed.strip_prefix('~') {
            Some(rest) => {
                let home = std::env::var("HOME").unwrap_or_default();
                PathBuf::from(format!("{home}{rest}"))
            }
            None => super::lexical_path::normalize(&cwd.join(trimmed)),
        };
        let shown = resolved.to_string_lossy().into_owned();
        if !resolved.exists() {
            bail!("Marketplace source path does not exist: {shown}");
        }
        if resolved.is_file() {
            if !shown.ends_with(".json") {
                bail!("Marketplace file must be a .json file: {shown}");
            }
            return Ok(source("file", &[("path", Some(shown))]));
        }
        if resolved.is_dir() {
            return Ok(source("directory", &[("path", Some(shown))]));
        }
        bail!("Marketplace source path is not a file or directory: {shown}");
    }
    if trimmed.contains('/') && !trimmed.contains(':') {
        // TS splitGitHubShorthand：`owner/repo#ref` 或 `owner/repo@ref`。
        let cut = trimmed.rfind('#').max(trimmed.rfind('@'));
        let (repo, r#ref) = match cut {
            Some(at) if at > 0 => (trimmed[..at].to_owned(), Some(trimmed[at + 1..].to_owned())),
            _ => (trimmed.to_owned(), None),
        };
        return Ok(source("github", &[("repo", Some(repo)), ("ref", r#ref)]));
    }
    bail!("Unsupported marketplace source: {input}")
}

// ---- 加载市场 manifest（TS loadMarketplaceFromSource，persist:false） ----

struct Loaded {
    /// 规范化后的原文：`name` trim、`plugins` 对象写法转为数组（原位），其余 key 原样。
    raw: Json,
    name: String,
    description: Option<String>,
    plugin_count: usize,
    source_root: Option<PathBuf>,
    temp: Option<PathBuf>,
}

/// TS `parseRequiredMarketplaceManifest` + `normalizeMarketplaceManifest`。
fn normalize(value: Json, required: bool) -> Result<Loaded> {
    if !value.is_object() {
        bail!("Marketplace manifest is invalid");
    }
    let name = value
        .get("name")
        .and_then(Json::as_str)
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
    if required && !valid {
        bail!("Marketplace manifest is invalid");
    }
    let plugins: Vec<Json> = match value.get("plugins") {
        Some(Json::Array(items)) => items.clone(),
        Some(Json::Object(entries)) => entries
            .iter()
            .map(|(key, plugin)| {
                let mut entry = Json::object();
                entry.set("name", Json::str(key));
                if let Json::Object(fields) = plugin {
                    for (k, v) in fields {
                        entry.set(k, v.clone());
                    }
                }
                entry
            })
            .collect(),
        _ => vec![],
    };
    let plugin_count = plugins
        .iter()
        .filter(|p| {
            p.is_object()
                && p.get("name")
                    .and_then(Json::as_str)
                    .is_some_and(|n| !n.trim().is_empty())
        })
        .count();
    let description = value
        .get("description")
        .and_then(Json::as_str)
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|m| m.get("description"))
                .and_then(Json::as_str)
        })
        .map(str::to_owned);
    let mut raw = value;
    if required {
        raw.set("name", Json::str(name.clone()));
        raw.set("plugins", Json::Array(plugins));
    }
    Ok(Loaded {
        raw,
        name,
        description,
        plugin_count,
        source_root: None,
        temp: None,
    })
}

/// TS `findMarketplaceManifestPath`：显式路径 > `.claude-plugin/marketplace.json` > `marketplace.json`。
fn find_manifest(root: &Path, explicit: Option<&str>) -> Option<PathBuf> {
    let base = super::lexical_path::normalize(root);
    explicit
        .into_iter()
        .chain([".claude-plugin/marketplace.json", "marketplace.json"])
        .map(|candidate| super::lexical_path::normalize(&base.join(candidate)))
        .find(|path| path.starts_with(&base) && path.is_file())
}

fn read_json_file(path: &Path) -> Result<Json> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Unable to read {}", path.display()))?;
    Json::parse(&text).ok_or_else(|| anyhow!("Unexpected token in JSON at {}", path.display()))
}

/// TS `requestMarketplaceJson`：手动重定向（跨源丢弃自定义头）、10 MiB / 180 s。
async fn request_json(url: &str, headers: Vec<(String, String)>) -> Result<Json> {
    let mut current = url.to_owned();
    let mut headers = headers;
    for _ in 0..=JSON_MAX_REDIRECTS {
        let client = super::web_fetch::proxied_client(current.clone(), JSON_TIMEOUT).await?;
        let mut request = client.get(&current);
        for (key, value) in &headers {
            request = request.header(key, value);
        }
        let mut response = request.send().await?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    anyhow!("Marketplace redirect is missing Location header: {current}")
                })?;
            let base = url::Url::parse(&current)?;
            let next = base.join(location)?;
            if next.origin() != base.origin() {
                headers.clear();
            }
            current = next.to_string();
            continue;
        }
        if !(200..300).contains(&status) {
            let reason = response.status().canonical_reason().unwrap_or_default();
            bail!("Failed to fetch marketplace: {status} {reason}");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > JSON_MAX_BYTES {
                bail!("Marketplace response exceeds {JSON_MAX_BYTES} bytes");
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&body);
        return Json::parse(&text).ok_or_else(|| anyhow!("Unexpected token in JSON at position 0"));
    }
    bail!("Marketplace fetch exceeded redirect limit: {url}")
}

async fn load(source: &Json) -> Result<Loaded> {
    let text = |key: &str| source.get(key).and_then(Json::as_str).map(str::to_owned);
    let kind = text("source").unwrap_or_default();
    match kind.as_str() {
        "settings" => normalize(
            source
                .get("marketplace")
                .cloned()
                .unwrap_or_else(Json::object),
            false,
        ),
        "file" => {
            let path = PathBuf::from(text("path").unwrap_or_default());
            let mut loaded = normalize(read_json_file(&path)?, true)?;
            loaded.source_root = path.parent().map(Path::to_owned);
            Ok(loaded)
        }
        "directory" => {
            let dir = PathBuf::from(text("path").unwrap_or_default());
            let file = find_manifest(&dir, None).ok_or_else(|| {
                anyhow!(
                    "Marketplace manifest not found in directory: {}",
                    dir.display()
                )
            })?;
            let mut loaded = normalize(read_json_file(&file)?, true)?;
            loaded.source_root = Some(dir);
            Ok(loaded)
        }
        "url" => {
            let headers = match source.get("headers") {
                Some(Json::Object(entries)) => entries
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                    .collect(),
                _ => vec![],
            };
            normalize(
                request_json(&text("url").unwrap_or_default(), headers).await?,
                true,
            )
        }
        "github" | "git" => {
            let (url, label) = if kind == "github" {
                let repo = text("repo").unwrap_or_default();
                (
                    format!("https://github.com/{repo}.git"),
                    format!("GitHub repo: {repo}"),
                )
            } else {
                let url = text("url").unwrap_or_default();
                (url.clone(), format!("git repo: {url}"))
            };
            let root = plugin_git::resolve(plugin_git::RepoSource {
                url,
                path: None,
                r#ref: text("ref"),
                sha: None,
            })
            .await?;
            let result = (|| {
                let file = find_manifest(&root.path, text("path").as_deref())
                    .ok_or_else(|| anyhow!("Marketplace manifest not found in {label}"))?;
                normalize(read_json_file(&file)?, true)
            })();
            match result {
                Ok(mut loaded) => {
                    loaded.source_root = Some(root.path.clone());
                    loaded.temp = Some(root.temp_dir());
                    Ok(loaded)
                }
                Err(error) => {
                    let _ = std::fs::remove_dir_all(root.temp_dir());
                    Err(error)
                }
            }
        }
        "npm" | "hostPattern" | "pathPattern" => {
            bail!("Marketplace {UNSUPPORTED}: {kind}")
        }
        other => bail!("Marketplace {UNSUPPORTED}: {other}"),
    }
}

/// TS `addMarketplace`：加载 → 官方 id 守卫 → 激活目录（与 known 记录同一 transactionId）→ upsert known。
pub(super) async fn add(
    storage: &Path,
    source: &Json,
    expected_id: Option<&str>,
    trusted_id: Option<&str>,
) -> Result<Json> {
    let loaded = load(source).await?;
    let result = commit(storage, source, &loaded, expected_id, trusted_id);
    if let Some(temp) = &loaded.temp {
        let _ = std::fs::remove_dir_all(temp);
    }
    result
}

fn commit(
    storage: &Path,
    source: &Json,
    loaded: &Loaded,
    expected_id: Option<&str>,
    trusted_id: Option<&str>,
) -> Result<Json> {
    let name = loaded.name.as_str();
    if name == OFFICIAL_MARKETPLACE && Some(name) != trusted_id {
        bail!(
            "Cannot add a marketplace named \"{name}\": that id is reserved for the official marketplace."
        );
    }
    if let Some(expected) = expected_id
        && name != expected
    {
        bail!("Marketplace declaration id mismatch: expected {expected}, received {name}");
    }
    if trusted_id == Some(OFFICIAL_MARKETPLACE) && name != OFFICIAL_MARKETPLACE {
        bail!("Official marketplace source must provide {OFFICIAL_MARKETPLACE}, received {name}");
    }
    let mut plugin_count = loaded.plugin_count;
    let authority = known_path(storage);
    let target = storage.join("marketplaces").join(sanitize(name));
    let write_manifest = |staged: &Path| -> Result<()> {
        std::fs::write(
            staged.join("marketplace.json"),
            format!("{}\n", loaded.raw.pretty()),
        )?;
        Ok(())
    };
    let activation = if name == OFFICIAL_MARKETPLACE {
        // 官方市场：写 CDN 分片并与内置分片合并；记录数取合并后的目录。
        let merged = super::official_plugins_marketplace::write_cdn(storage, &loaded.raw)?;
        plugin_count = normalize(merged, true)?.plugin_count;
        None
    } else {
        Some(atomic_dir::activate(
            loaded.source_root.as_deref(),
            &target,
            &authority,
            write_manifest,
        )?)
    };
    let now = iso_now();
    let mut record = Json::object();
    record.set("id", Json::str(name));
    record.set("source", source.clone());
    record.set("name", Json::str(name));
    if let Some(description) = loaded.description.as_deref().filter(|d| !d.is_empty()) {
        record.set("description", Json::str(description));
    }
    record.set("addedAt", Json::str(now.clone()));
    record.set("lastUpdated", Json::str(now));
    record.set("pluginCount", Json::Number(plugin_count.into()));
    if let Some(activation) = &activation {
        record.set(
            "cacheTransactionId",
            Json::str(activation.transaction_id.clone()),
        );
    }
    // TS upsertKnownMarketplace：已有记录去掉旧 cacheTransactionId / lastRefreshFailure 后原地覆盖，保留 addedAt。
    let mut records = known(storage);
    let merged = match records.iter_mut().find(|r| id_of(r) == name) {
        Some(existing) => {
            let added = existing.get("addedAt").cloned();
            existing.remove("cacheTransactionId");
            existing.remove("lastRefreshFailure");
            if let Json::Object(fields) = &record {
                for (key, value) in fields {
                    existing.set(key, value.clone());
                }
            }
            if let Some(added) = added {
                existing.set("addedAt", added);
            }
            existing.clone()
        }
        None => {
            records.push(record.clone());
            record.clone()
        }
    };
    if let Err(error) = write_known(storage, records) {
        if let Some(activation) = activation {
            activation.rollback()?;
        }
        return Err(error);
    }
    if let Some(activation) = activation {
        activation.finalize();
    }
    Ok(merged)
}

/// TS `toValidationDiagnostic`（刷新失败落盘用的 code）。
fn failure_code(error: &anyhow::Error) -> &'static str {
    if let Some(source) = error.downcast_ref::<plugin_git::SourceError>() {
        return source.code;
    }
    let message = error.to_string();
    if message.contains(UNSUPPORTED) {
        "plugin_marketplace_source_unsupported"
    } else if message.contains("Cross-marketplace dependency") {
        "plugin_dependency_cross_marketplace"
    } else if message.contains("dependency cycle") {
        "plugin_dependency_cycle"
    } else if message.contains("Dependency not found")
        || message.contains("Marketplace not found for dependency")
    {
        "plugin_dependency_missing"
    } else {
        "plugin_marketplace_invalid"
    }
}

/// TS `updateMarketplace`（单个已知市场）：用记录自带 source 受信任刷新；失败写 `lastRefreshFailure`。
async fn refresh(storage: &Path, id: &str) -> Result<Option<Json>> {
    let Some(record) = known(storage).into_iter().find(|r| id_of(r) == id) else {
        bail!("Marketplace not found: {id}");
    };
    let source = record.get("source").cloned().unwrap_or_else(Json::object);
    match add(storage, &source, None, Some(id)).await {
        Ok(updated) => Ok(Some(updated)),
        Err(error) => {
            let mut records = known(storage);
            if let Some(existing) = records.iter_mut().find(|r| id_of(r) == id) {
                let mut failure = Json::object();
                failure.set("code", Json::str(failure_code(&error)));
                failure.set("failedAt", Json::str(iso_now()));
                failure.set("message", Json::str(error.to_string()));
                existing.set("lastRefreshFailure", failure);
                write_known(storage, records)?;
            }
            Ok(None)
        }
    }
}

/// TS `toMarketplaceSummaryData(record)`（无 featured / 可见数投影）。
fn summary(record: &Json) -> Value {
    let record: Value = serde_json::from_str(&record.compact()).unwrap_or_default();
    let id = record["id"].as_str().unwrap_or_default();
    let mut out = json!({
        "id": id,
        "name": record["name"],
        "source": record["source"],
        "pluginCount": record["pluginCount"],
        "isOfficial": id == OFFICIAL_MARKETPLACE,
    });
    for key in ["description", "lastUpdated"] {
        if record[key].as_str().is_some_and(|s| !s.is_empty()) {
            out[key] = record[key].clone();
        }
    }
    let failure = &record["lastRefreshFailure"];
    if failure.is_object() {
        out["refreshFailure"] = json!({
            "code": failure["code"],
            "failedAt": failure["failedAt"],
            "message": failure["message"],
        });
    }
    out
}

// ---- 协议入口 ----

async fn context(params: &Value) -> Result<(PathBuf, PathBuf, Value)> {
    let cwd = super::plugin_list::workspace_path(params)?;
    let config = super::extension_config::load(&cwd).await?;
    let storage = super::extension_config::storage(&config);
    Ok((cwd, storage, config))
}

pub(super) async fn add_params(params: &Value) -> Result<Value> {
    let source_input = super::plugin_list::non_empty(params, "source")?.to_owned();
    let (cwd, storage, _) = context(params).await?;
    let _guard = super::plugin_uninstall::storage_lock(&storage).await;
    let source = parse_source_input(&source_input, &cwd)?;
    if params["dryRun"] == true {
        let source: Value = serde_json::from_str(&source.compact())?;
        return Ok(json!({
            "marketplace": {"id": "dry-run", "name": "dry-run", "source": source, "pluginCount": 0, "isOfficial": false},
            "diagnostics": [],
        }));
    }
    let record = add(&storage, &source, None, None).await?;
    Ok(json!({ "marketplace": summary(&record), "diagnostics": [] }))
}

pub(super) async fn remove_params(params: &Value) -> Result<Value> {
    let id = super::plugin_list::non_empty(params, "marketplace")?.to_owned();
    let (_, storage, _) = context(params).await?;
    let _guard = super::plugin_uninstall::storage_lock(&storage).await;
    let records: Vec<Json> = known(&storage)
        .into_iter()
        .filter(|r| id_of(r) != id)
        .collect();
    write_known(&storage, records)?;
    Ok(json!({ "diagnostics": [] }))
}

/// TS `updateZCodePluginMarketplace`：指定 id 时只刷新它（声明未物化则按声明 add）；否则刷新全部已知市场。
pub(super) async fn update_params(params: &Value) -> Result<Value> {
    let only = params["marketplace"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let (_, storage, _) = context(params).await?;
    let _guard = super::plugin_uninstall::storage_lock(&storage).await;
    super::plugin_marketplace::ensure_default_marketplaces(&storage)?;
    let user = super::extension_config::load_user().await?;
    let declared: Vec<(String, Json)> = user["plugins"]["extraKnownMarketplaces"]
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .filter(|(_, d)| d["source"].is_object())
                .filter_map(|(id, d)| Some((id.clone(), Json::parse(&d["source"].to_string())?)))
                .collect()
        })
        .unwrap_or_default();
    let known_ids: Vec<String> = known(&storage)
        .iter()
        .map(|r| id_of(r).to_owned())
        .collect();
    if let Some(id) = &only
        && !known_ids.contains(id)
        && !declared.iter().any(|(d, _)| d == id)
    {
        bail!("Marketplace not found: {id}");
    }
    let targets: Vec<String> = match &only {
        Some(id) => vec![id.clone()],
        None => known_ids.clone(),
    };
    let mut updated = vec![];
    let mut diagnostics = vec![];
    for id in targets {
        let declared_source = declared.iter().find(|(d, _)| d == &id).map(|(_, s)| s);
        let known_record = known(&storage).into_iter().find(|r| id_of(r) == id);
        if only.is_some()
            && let (Some(declared), Some(record)) = (declared_source, &known_record)
            && record.get("source").map(Json::compact) != Some(declared.compact())
        {
            diagnostics.push(json!({
                "code": "plugin_marketplace_invalid",
                "message": format!("Workspace marketplace declaration \"{id}\" conflicts with an existing Host source. Remove the existing marketplace or use a different marketplace id before materializing it."),
                "severity": "error",
                "pluginId": id,
            }));
            continue;
        }
        if let (Some(declared), None) = (declared_source, &known_record) {
            match add(&storage, declared, Some(&id), None).await {
                Ok(record) => updated.push(record),
                Err(error) => diagnostics.push(json!({
                    "code": error.downcast_ref::<plugin_git::SourceError>().map_or("plugin_marketplace_invalid", |e| e.code),
                    "message": error.to_string(),
                    "severity": "error",
                    "pluginId": id,
                })),
            }
            continue;
        }
        if let Some(record) = refresh(&storage, &id).await? {
            updated.push(record);
        }
    }
    for record in known(&storage) {
        let id = id_of(&record);
        if only.as_deref().is_some_and(|o| o != id) {
            continue;
        }
        if let Some(failure) = record.get("lastRefreshFailure").filter(|f| f.is_object()) {
            diagnostics.push(json!({
                "code": failure.get("code").and_then(Json::as_str),
                "message": failure.get("message").and_then(Json::as_str),
                "severity": "error",
                "pluginId": id,
            }));
        }
    }
    Ok(json!({
        "marketplaces": updated.iter().map(summary).collect::<Vec<_>>(),
        "diagnostics": diagnostics,
    }))
}

/// TS `ensureMarketplaceManifestAvailable`：本地没有 manifest 但有已知记录时，用记录 source 受信任拉取。
pub(super) async fn ensure_manifest(storage: &Path, marketplace: &str) -> Result<()> {
    if super::plugin_marketplace::manifest(storage, marketplace).is_some() {
        return Ok(());
    }
    let Some(record) = known(storage).into_iter().find(|r| id_of(r) == marketplace) else {
        return Ok(());
    };
    let source = record.get("source").cloned().unwrap_or_else(Json::object);
    add(storage, &source, None, Some(marketplace))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_input_forms() {
        let cwd = std::env::temp_dir();
        assert_eq!(
            parse_source_input("https://example.com/m.json", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"url","url":"https://example.com/m.json"}"#
        );
        assert_eq!(
            parse_source_input("https://github.com/a/b#main", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"git","url":"https://github.com/a/b.git","ref":"main"}"#
        );
        assert_eq!(
            parse_source_input("git@github.com:a/b.git", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"git","url":"git@github.com:a/b.git"}"#
        );
        assert_eq!(
            parse_source_input("acme/market@v1", &cwd)
                .unwrap()
                .compact(),
            r#"{"source":"github","repo":"acme/market","ref":"v1"}"#
        );
        assert!(parse_source_input("  ", &cwd).is_err());
        assert!(parse_source_input("nonsense", &cwd).is_err());
    }
}
