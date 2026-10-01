//! 插件 zip 源（docs/specs/rust-plugin-marketplace-write.md W2），对齐 TS `adapters/src/plugins/zip-source.ts`：
//! HTTPS（或回环 HTTP）下载、手动跟随重定向（跨源丢弃自定义头）、sha256 校验、安全解压与插件根定位。
//! 解压到系统临时目录，调用方激活缓存后删除。

use crate::domain::json_order::Json;
use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DOWNLOAD_MAX_BYTES: usize = 200 * 1024 * 1024;
const EXTRACT_MAX_BYTES: u64 = 500 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_SINGLE_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_millis(180_000);
const DENIED_HEADERS: [&str; 4] = [
    "authorization",
    "cookie",
    "proxy-authorization",
    "set-cookie",
];

/// 解压出的插件根；`temp` 是整个临时目录，用完调用 [`ZipRoot::cleanup`]。
pub(super) struct ZipRoot {
    pub path: PathBuf,
    temp: PathBuf,
}

impl ZipRoot {
    pub(super) fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}

struct Source {
    url: String,
    sha256: String,
    path: Option<String>,
    strip_root: Option<bool>,
    headers: Vec<(String, String)>,
}

/// TS `readRequired*` / `readOptional*` 与 `validateZipSourceInput`。
fn parse_source(source: &Json) -> Result<Source> {
    let url = source
        .get("url")
        .and_then(Json::as_str)
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| anyhow!("Plugin URL source requires a non-empty url"))?
        .to_owned();
    // 读取顺序同 TS（headers → path → sha256 → stripRoot），第一处错误一致。
    let headers = match source.get("headers") {
        None => vec![],
        Some(Json::Object(entries)) => entries
            .iter()
            .map(|(key, value)| match value {
                Json::String(value) => Ok((key.clone(), value.clone())),
                _ => Err(anyhow!("Plugin zip source header must be a string: {key}")),
            })
            .collect::<Result<_>>()?,
        Some(_) => bail!("Plugin zip source headers must be an object"),
    };
    let path = match source.get("path") {
        None => None,
        Some(Json::String(path)) => Some(path.clone()),
        Some(_) => bail!("Plugin zip source path must be a string"),
    };
    let sha256 = match source.get("sha256") {
        Some(Json::String(sha)) => sha.clone(),
        _ => bail!("Plugin zip source sha256 is required"),
    };
    let strip_root = match source.get("stripRoot") {
        None => None,
        Some(Json::Bool(value)) => Some(*value),
        Some(_) => bail!("Plugin zip source stripRoot must be a boolean"),
    };
    validate_url(&url)?;
    let lower = sha256.to_lowercase();
    if lower.len() != 64 || !lower.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Plugin zip source sha256 must be a 64 character hex string");
    }
    for (key, _) in &headers {
        if DENIED_HEADERS.contains(&key.to_lowercase().as_str()) {
            bail!("Plugin zip source header is not allowed: {key}");
        }
    }
    if let Some(path) = &path {
        normalize_relative(path)?;
    }
    Ok(Source {
        url,
        sha256: lower,
        path,
        strip_root,
        headers,
    })
}

/// TS `validateZipDownloadUrl`：只允许 HTTPS，回环地址允许 HTTP。
fn validate_url(value: &str) -> Result<()> {
    let url =
        url::Url::parse(value).map_err(|_| anyhow!("Plugin zip source URL is invalid: {value}"))?;
    if url.scheme() == "https" {
        return Ok(());
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() == "http" && loopback {
        return Ok(());
    }
    bail!("Plugin zip source URL must be HTTPS: {value}")
}

/// TS `normalizeZipRelativePath`：拒绝空、反斜杠、绝对路径、盘符、`.` / `..` 段；去掉尾部 `/`。
fn normalize_relative(path: &str) -> Result<String> {
    let unsafe_path = || anyhow!("Unsafe plugin zip path: {path}");
    if path.contains('\0') {
        return Err(unsafe_path());
    }
    let trimmed = path.trim_end_matches('/');
    let drive = trimmed.len() >= 2
        && trimmed.as_bytes()[0].is_ascii_alphabetic()
        && trimmed.as_bytes()[1] == b':';
    if trimmed.is_empty() || path.contains('\\') || path.starts_with('/') || drive {
        return Err(unsafe_path());
    }
    if trimmed
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(unsafe_path());
    }
    Ok(trimmed.to_owned())
}

/// TS `downloadZipArchive`。
async fn download(source: &Source) -> Result<Vec<u8>> {
    let mut current = source.url.clone();
    let mut headers = source.headers.clone();
    for _ in 0..=MAX_REDIRECTS {
        validate_url(&current)?;
        let client = super::web_fetch::proxied_client(current.clone(), DOWNLOAD_TIMEOUT).await?;
        let mut request = client.get(&current);
        for (key, value) in &headers {
            request = request.header(key, value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| anyhow!("Failed to download plugin zip: {e}"))?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    anyhow!("Plugin zip download redirect is missing Location header: {current}")
                })?;
            let base = url::Url::parse(&current)?;
            let next = base.join(location)?;
            // 跨源继续发市场自定义头会把内部元数据泄露给跳转目标。
            if next.origin() != base.origin() {
                headers.clear();
            }
            current = next.to_string();
            continue;
        }
        if !(200..300).contains(&status) {
            let reason = response.status().canonical_reason().unwrap_or_default();
            bail!("Failed to download plugin zip: {status} {reason}");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > DOWNLOAD_MAX_BYTES {
                bail!("Plugin zip download exceeds {DOWNLOAD_MAX_BYTES} bytes");
            }
            body.extend_from_slice(&chunk);
        }
        return Ok(body);
    }
    bail!(
        "Plugin zip download exceeded redirect limit: {}",
        source.url
    )
}

/// TS `extractZipArchive` + `classifyZipEntry`：返回顶层段集合（保持首次出现的顺序）。
fn extract(bytes: &[u8], target: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(target)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .context("Failed to open plugin zip archive")?;
    if archive.len() > MAX_ENTRIES {
        bail!(
            "Plugin zip has too many entries: {}/{MAX_ENTRIES}",
            MAX_ENTRIES + 1
        );
    }
    let mut top: Vec<String> = vec![];
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let normalized = normalize_relative(&name)?;
        let first = normalized
            .split('/')
            .next()
            .unwrap_or(&normalized)
            .to_owned();
        if !top.contains(&first) {
            top.push(first);
        }
        if entry.encrypted() {
            bail!("Encrypted plugin zip entries are not supported: {name}");
        }
        let file_type = entry.unix_mode().map_or(0, |mode| mode & 0o170000);
        if file_type == 0o120000 {
            bail!("Plugin zip entry symlinks are not supported: {name}");
        }
        if file_type != 0 && file_type != 0o100000 && file_type != 0o040000 {
            bail!("Unsupported plugin zip entry type: {name}");
        }
        let path = target.join(normalized.split('/').collect::<PathBuf>());
        if file_type == 0o040000 || name.ends_with('/') {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if entry.size() > MAX_SINGLE_FILE_BYTES {
            bail!("Plugin zip entry exceeds single file limit: {name}");
        }
        let mut content = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(&mut entry, MAX_SINGLE_FILE_BYTES + 1),
            &mut content,
        )?;
        if content.len() as u64 > MAX_SINGLE_FILE_BYTES {
            bail!("Plugin zip entry exceeds single file limit: {name}");
        }
        total += content.len() as u64;
        if total > EXTRACT_MAX_BYTES {
            bail!("Plugin zip extracted content exceeds limit: {total}/{EXTRACT_MAX_BYTES}");
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
    }
    Ok(top)
}

fn has_manifest(root: &Path) -> bool {
    [".zcode-plugin", ".claude-plugin", ".codex-plugin"]
        .iter()
        .any(|dir| root.join(dir).join("plugin.json").is_file())
}

/// TS `resolveZipRoot`：显式 `path` > 解压根已有 manifest > `stripRoot`（缺省 true）且只有一个顶层目录 > 解压根。
fn resolve_root(extract: &Path, source: &Source, top: &[String]) -> Result<PathBuf> {
    if let Some(path) = &source.path {
        let requested = extract.join(normalize_relative(path)?.split('/').collect::<PathBuf>());
        if !requested.is_dir() {
            bail!("Plugin zip source subdirectory does not exist: {path}");
        }
        return Ok(requested);
    }
    if has_manifest(extract) {
        return Ok(extract.to_owned());
    }
    if source.strip_root != Some(false)
        && let [single] = top
    {
        let candidate = extract.join(single);
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    if !extract.is_dir() {
        bail!("Plugin zip did not extract a plugin root directory");
    }
    Ok(extract.to_owned())
}

/// TS `resolveZipPluginSource`：下载、校验、解压，返回插件根（失败时清理临时目录）。
pub(super) async fn resolve(source: &Json) -> Result<ZipRoot> {
    let source = parse_source(source)?;
    let temp = std::env::temp_dir().join(format!("zcode-plugin-zip-{}", uuid::Uuid::new_v4()));
    let result = async {
        let bytes = download(&source).await?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != source.sha256 {
            bail!(
                "Plugin zip sha256 mismatch: expected={}, actual={actual}",
                source.sha256
            );
        }
        std::fs::create_dir_all(&temp)?;
        std::fs::write(temp.join("source.zip"), &bytes)?;
        let extract_root = temp.join("extract");
        let top = extract(&bytes, &extract_root)?;
        resolve_root(&extract_root, &source, &top)
    }
    .await;
    match result {
        Ok(path) => Ok(ZipRoot { path, temp }),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_reject_escapes() {
        assert_eq!(normalize_relative("a/b/").unwrap(), "a/b");
        for bad in ["", "/a", "a\\b", "C:/x", "a/../b", "./a", "a//b"] {
            assert!(normalize_relative(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn urls_require_https_except_loopback() {
        assert!(validate_url("https://cdn.example/p.zip").is_ok());
        assert!(validate_url("http://127.0.0.1:8080/p.zip").is_ok());
        assert!(validate_url("http://localhost/p.zip").is_ok());
        assert!(validate_url("http://[::1]/p.zip").is_ok());
        assert!(validate_url("http://example.com/p.zip").is_err());
        assert!(validate_url("file:///p.zip").is_err());
    }
}
