//! Built-in 来源：Bundled（随包，可信基线）+ Active/LKG（按 平台/应用版本/Endpoint 隔离的可丢弃缓存）。
//!
//! 对应 TS `escode-builtin-cache-paths.ts` 与 `NodeESCodeBuiltinProviderConfigSource` 的本地部分
//! （docs/specs/rust-u2-providers.md §11，P5a）。Remote 下载与租约属于 P5b，这里不涉及。

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use escode_cli_host::file_lock;
use escode_host_control::logging;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::endpoint::EndpointSource;
use super::files;

/// Active 缓存的定位输入（TS `escodeBuiltinEnvironment`）：Endpoint 每次读取时动态解析（spec §12）。
#[derive(Debug, Clone)]
pub struct ActiveCache {
    pub root: PathBuf,
    pub app_version: String,
    pub endpoint: EndpointSource,
}

/// TS `resolveESCodeBuiltinClientPlatform`：`<windows|darwin|linux…>-<aarch64|x86_64|…>`。
pub fn client_platform() -> String {
    let target = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        other => other,
    };
    format!("{target}-{}", std::env::consts::ARCH)
}

/// TS `normalizeESCodeBuiltinEndpointOrigin`：只接受 HTTP(S)，取 `URL.origin`。
pub fn normalize_endpoint_origin(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("ESCode Built-in Endpoint Origin 不能为空");
    }
    let url = url::Url::parse(trimmed)?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("ESCode Built-in Endpoint Origin 只支持 HTTP(S)");
    }
    Ok(url.origin().ascii_serialization())
}

/// TS `createESCodeBuiltinEndpointKey`：`endpoint-` + sha256(规范化 Origin) 前 32 位十六进制。
pub fn endpoint_key(origin: &str) -> Result<String> {
    let digest = Sha256::digest(normalize_endpoint_origin(origin)?.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("endpoint-{}", &hex[..32]))
}

fn segment(value: &str, name: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\']) {
        bail!("ESCode Built-in {name} 不是合法路径段");
    }
    Ok(value.to_owned())
}

/// TS `resolveESCodeBuiltinCachePaths().activeFilePath`。
pub fn active_file_path(
    root: &Path,
    app_version: &str,
    endpoint_origin: &str,
    platform: &str,
) -> Result<PathBuf> {
    Ok(root
        .join("runtime")
        .join("provider")
        .join(segment(platform, "platform")?)
        .join(segment(app_version, "appVersion")?)
        .join(endpoint_key(endpoint_origin)?)
        .join("escode-builtin.json"))
}

/// 刷新控制文件与 Active 同目录（TS `controlFilePath`）。
pub fn control_file_path(active: &Path) -> PathBuf {
    active.with_file_name("escode-builtin-refresh.json")
}

/// TS `ApplyESCodeBuiltinReleaseResult`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyResult {
    Updated,
    Unchanged,
    Stale,
}

pub struct BuiltinSource {
    bundled: PathBuf,
    cache: Option<ActiveCache>,
    platform: String,
}

/// 一次解析的结果：当前 Endpoint 与对应的 Active 路径。
#[derive(Debug, Clone)]
pub struct ResolvedActive {
    pub endpoint_origin: String,
    pub path: PathBuf,
}

/// 候选：`None` = 文件不存在；`Err` = 存在但无效。
type Candidate = Option<Result<Value, String>>;

impl BuiltinSource {
    /// `cache` 缺省 = 只读 Bundled（与 P1 行为一致）。
    pub fn new(bundled: PathBuf, cache: Option<ActiveCache>) -> Self {
        Self {
            bundled,
            cache,
            platform: client_platform(),
        }
    }

    /// 在已有 Bundled 路径上启用 Active 缓存。
    pub fn with_cache(&self, cache: ActiveCache) -> Self {
        Self::new(self.bundled.clone(), Some(cache))
    }

    pub fn app_version(&self) -> Option<&str> {
        self.cache.as_ref().map(|cache| cache.app_version.as_str())
    }

    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// 当前 Endpoint 与 Active 路径；未启用缓存、Endpoint 非法或路径段非法 → `Ok(None)`/`Err`。
    pub async fn resolve_active(&self) -> Result<Option<ResolvedActive>> {
        let Some(cache) = &self.cache else {
            return Ok(None);
        };
        let endpoint_origin = cache.endpoint.resolve().await?;
        let path = active_file_path(
            &cache.root,
            &cache.app_version,
            &endpoint_origin,
            &self.platform,
        )?;
        Ok((path != self.bundled).then_some(ResolvedActive {
            endpoint_origin,
            path,
        }))
    }

    /// 参与变更检测的文件（指纹轮询用）：Bundled、当前 Active、设置文件（Endpoint 覆盖）。
    pub async fn watched_files(&self) -> Vec<PathBuf> {
        let mut files = vec![self.bundled.clone()];
        match self.resolve_active().await {
            Ok(Some(active)) => files.push(active.path),
            Ok(None) => {}
            Err(error) => warn_disabled(&error),
        }
        if let Some(settings) = self
            .cache
            .as_ref()
            .and_then(|cache| cache.endpoint.settings_file())
        {
            files.push(settings);
        }
        files
    }

    /// TS `read()`：锁内选择并物化；缓存边界失败时绕过 Active，Bundled 自身无效才报错。
    pub async fn read(&self) -> Result<Value> {
        let active = match self.resolve_active().await {
            Ok(active) => active,
            Err(error) => {
                warn_disabled(&error);
                None
            }
        };
        let Some(active) = active else {
            return select(read_candidate(&self.bundled).await, None);
        };
        match self.read_materialized(&active.path).await {
            Ok(value) => Ok(value),
            Err(error) => {
                logging::warn(
                    "provider",
                    format!("builtin active cache unavailable, using bundled: {error:#}"),
                );
                select(read_candidate(&self.bundled).await, None)
            }
        }
    }

    /// TS `applyRemoteRelease`：锁内比较 revision，只前进；同 revision 内容不同是错误。
    pub async fn apply_remote_release(&self, release: Value) -> Result<ApplyResult> {
        files::parse_builtin(&release)?;
        let active = self
            .resolve_active()
            .await?
            .ok_or_else(|| anyhow::anyhow!("ESCode Built-in Active 缓存未启用"))?;
        if let Some(dir) = active.path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let guard = file_lock::acquire(&active.path).await?;
        let outcome = self.apply_locked(&active.path, release).await;
        guard.release().await;
        outcome
    }

    async fn apply_locked(&self, active: &Path, release: Value) -> Result<ApplyResult> {
        let current = self.select_and_write(active).await?;
        let (new, old) = (revision(&release), revision(&current));
        if new < old {
            return Ok(ApplyResult::Stale);
        }
        if new == old {
            if release == current {
                return Ok(ApplyResult::Unchanged);
            }
            bail!("ESCode Built-in 相同 revision {new} 对应不同内容");
        }
        file_lock::write_private(active, &serde_json::to_string_pretty(&release)?).await?;
        Ok(ApplyResult::Updated)
    }

    async fn read_materialized(&self, active: &Path) -> Result<Value> {
        if let Some(dir) = active.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let guard = file_lock::acquire(active).await?;
        let outcome = self.select_and_write(active).await;
        guard.release().await;
        outcome
    }

    async fn select_and_write(&self, active: &Path) -> Result<Value> {
        let bundled = read_candidate(&self.bundled).await;
        let current = read_candidate(active).await;
        let selected = select(bundled, current.clone())?;
        let up_to_date = matches!(&current, Some(Ok(value)) if *value == selected);
        if !up_to_date {
            file_lock::write_private(active, &serde_json::to_string_pretty(&selected)?).await?;
        }
        Ok(selected)
    }
}

fn warn_disabled(error: &anyhow::Error) {
    logging::warn(
        "provider",
        format!("builtin active cache disabled: {error:#}"),
    );
}

async fn read_candidate(path: &Path) -> Candidate {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return Some(Err(error.to_string())),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Some(Err(error.to_string())),
    };
    Some(match files::parse_builtin(&value) {
        Ok(_) => Ok(value),
        Err(error) => Err(format!("{error:#}")),
    })
}

pub fn revision(value: &Value) -> u64 {
    value.get("revision").and_then(Value::as_u64).unwrap_or(0)
}

/// TS `selectReleaseCandidate`：同 revision 内容冲突 → Bundled；否则 revision 大者（平局取 Bundled）。
fn select(bundled: Candidate, active: Candidate) -> Result<Value> {
    let (bundled_ok, bundled_err) = split(bundled);
    let (active_ok, active_err) = split(active);
    if let (Some(b), Some(a)) = (&bundled_ok, &active_ok)
        && revision(b) == revision(a)
        && b != a
    {
        return Ok(b.clone());
    }
    match (bundled_ok, active_ok) {
        (Some(b), Some(a)) => Ok(if revision(&a) > revision(&b) { a } else { b }),
        (Some(only), None) | (None, Some(only)) => Ok(only),
        (None, None) => bail!(
            "Bundled 与 Active ESCode Built-in Release 均不可用: {}",
            [bundled_err, active_err]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

fn split(candidate: Candidate) -> (Option<Value>, Option<String>) {
    match candidate {
        Some(Ok(value)) => (Some(value), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    }
}
