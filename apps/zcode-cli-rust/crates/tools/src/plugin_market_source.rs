//! 市场源输入解析与市场 manifest 加载（TS `parseMarketplaceSourceInput` / `loadMarketplaceFromSource`，persist:false）。
#[allow(unused_imports)]
use super::plugin_market_write::*;

use super::plugin_git;
use crate::domain::json_order::Json;
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const JSON_MAX_BYTES: usize = 10 * 1024 * 1024;

pub(super) const JSON_MAX_REDIRECTS: usize = 5;

pub(super) const JSON_TIMEOUT: Duration = Duration::from_millis(180_000);

/// TS `UnsupportedMarketplaceSourceError` / `UnsupportedPluginSourceError` 的文案前缀（诊断归类用）。
pub(super) const UNSUPPORTED: &str = "source is recognized but not supported in this runtime";

pub(super) fn split_ref(input: &str) -> (String, Option<String>) {
    match input.rfind('#') {
        Some(at) => (input[..at].to_owned(), Some(input[at + 1..].to_owned())),
        None => (input.to_owned(), None),
    }
}

pub(super) fn source(kind: &str, fields: &[(&str, Option<String>)]) -> Json {
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

pub(super) struct Loaded {
    /// 规范化后的原文：`name` trim、`plugins` 对象写法转为数组（原位），其余 key 原样。
    pub raw: Json,
    pub name: String,
    pub description: Option<String>,
    pub plugin_count: usize,
    pub source_root: Option<PathBuf>,
    pub temp: Option<PathBuf>,
}

/// TS `parseRequiredMarketplaceManifest` + `normalizeMarketplaceManifest`。
pub(super) fn normalize(value: Json, required: bool) -> Result<Loaded> {
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
pub(super) fn find_manifest(root: &Path, explicit: Option<&str>) -> Option<PathBuf> {
    let base = super::lexical_path::normalize(root);
    explicit
        .into_iter()
        .chain([".claude-plugin/marketplace.json", "marketplace.json"])
        .map(|candidate| super::lexical_path::normalize(&base.join(candidate)))
        .find(|path| path.starts_with(&base) && path.is_file())
}

pub(super) fn read_json_file(path: &Path) -> Result<Json> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Unable to read {}", path.display()))?;
    Json::parse(&text).ok_or_else(|| anyhow!("Unexpected token in JSON at {}", path.display()))
}

/// TS `requestMarketplaceJson`：手动重定向（跨源丢弃自定义头）、10 MiB / 180 s。
pub(super) async fn request_json(url: &str, headers: Vec<(String, String)>) -> Result<Json> {
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

pub(super) async fn load(source: &Json) -> Result<Loaded> {
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
