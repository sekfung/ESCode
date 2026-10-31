//! Source Server release 清单与部署决策（S4-b，docs/specs/source-server-remote-rust-host.md §11.2/§11.3）。
//!
//! 与 TS `packages/server/src/remote/remoteAssetCache.ts` 的 component manifest 解析、
//! `remoteAssetDeployDecision.ts` 的版本部署判定、`remoteAssetLiveIdentity.ts` 的 SHA 身份判定
//! 逐项对齐。本切片只做纯解析与 dry-run 判定，不下载、不写远端。

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::source_server_transport::resolve_posix_home_path;

pub const RELEASE_MANIFEST_SCHEMA_VERSION: u8 = 1;
/// 远端 Source Server 根目录（与 TS `REMOTE_BASE` 一致，`~` 由 transport 展开）。
pub const REMOTE_SERVER_ROOT: &str = "~/.escode/server";
pub const REMOTE_ASSET_COMPONENT_META_DIR: &str = "~/.escode/server/.asset-components";

/// 只保留当前客户端认识的组件；未知 id 跳过但不阻断下载。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub schema_version: u8,
    pub app_version: String,
    pub platform_arch: String,
    pub components: Vec<ReleaseComponent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseComponent {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub artifact_path: String,
    pub mount: String,
}

impl ReleaseComponent {
    /// 落盘缓存目录使用的版本：内容寻址组件剥掉 `+<hash>` 后缀。
    pub fn cache_version(&self) -> &str {
        component_cache_version(&self.version)
    }

    /// `server-bundle` / `glm` 以 sha256 而不是语义版本作为缓存身份。
    pub fn uses_content_addressed_identity(&self) -> bool {
        matches!(self.id.as_str(), "server-bundle" | "glm")
    }
}

/// 组件在远端 `.asset-components` 中的存活身份（TS `RemoteAssetComponentMeta`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentMeta {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_refresh_app_version: Option<String>,
    pub platform_arch: String,
}

impl ComponentMeta {
    pub fn path(component_id: &str) -> String {
        format!("{REMOTE_ASSET_COMPONENT_META_DIR}/{component_id}.json")
    }

    /// 与 TS `writeRemoteAssetComponentMeta` 相同的行格式（字段顺序 + 末尾换行）。
    pub fn to_json_line(&self) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployDecision {
    pub should_deploy: bool,
    pub reason: Option<String>,
}

impl DeployDecision {
    fn deploy(reason: impl Into<String>) -> Self {
        Self {
            should_deploy: true,
            reason: Some(reason.into()),
        }
    }

    fn keep() -> Self {
        Self {
            should_deploy: false,
            reason: None,
        }
    }
}

/// TS `sourceServerInstallPreviewSchema` 的只读投影；`sizeBytes` 是安装包实际大小。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_path: Option<String>,
    pub size_bytes: u64,
}

pub struct DeployDecisionOptions<'a> {
    pub component_id: &'a str,
    pub platform_arch: &'a str,
    pub remote_path: &'a str,
    pub remote_exists: bool,
    pub expected_version: Option<&'a str>,
    pub force: bool,
    pub fallback_deploy_when_version_unknown: bool,
    pub remote_meta: Option<&'a ComponentMeta>,
}

pub fn expected_mount(component_id: &str, platform_arch: &str) -> Option<String> {
    match component_id {
        "server-bundle" => Some("server".to_owned()),
        "node-runtime" => Some(format!("node/{platform_arch}")),
        "node-pty" => Some(format!("node-pty/{platform_arch}")),
        "glm" => Some(format!("glm/{platform_arch}")),
        "bfs" | "ripgrep" | "ugrep" => Some(format!("tools/{platform_arch}/{component_id}")),
        _ => None,
    }
}

/// 严格解析 `manifest-<platformArch>.json`；错误信息与 TS 诊断同义。
pub fn parse_release_manifest(
    value: &Value,
    expected_app_version: &str,
    expected_platform_arch: &str,
) -> Result<ReleaseManifest> {
    let manifest = value.as_object().ok_or_else(|| {
        anyhow::anyhow!("[remote-assets] invalid manifest payload: expect object")
    })?;
    let schema_version = required_number(manifest, "schemaVersion", "manifest")?;
    if schema_version.fract() != 0.0 || schema_version != f64::from(RELEASE_MANIFEST_SCHEMA_VERSION)
    {
        bail!("[remote-assets] unsupported manifest schemaVersion={schema_version}");
    }
    let app_version = required_string(manifest, "appVersion", "manifest")?;
    if app_version != expected_app_version {
        bail!(
            "[remote-assets] manifest appVersion mismatch: expected={expected_app_version}, actual={app_version}"
        );
    }
    let platform_arch = required_string(manifest, "platformArch", "manifest")?;
    if platform_arch != expected_platform_arch {
        bail!(
            "[remote-assets] manifest platformArch mismatch: expected={expected_platform_arch}, actual={platform_arch}"
        );
    }
    let raw_components = manifest
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("[remote-assets] manifest.components must be array"))?;
    if raw_components.is_empty() {
        bail!("[remote-assets] manifest.components is empty");
    }

    let mut components = Vec::new();
    let mut seen_ids = std::collections::BTreeSet::new();
    for (index, raw_component) in raw_components.iter().enumerate() {
        let context = format!("manifest.components[{index}]");
        let component = raw_component
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("[remote-assets] {context} must be object"))?;
        let id = required_string(component, "id", &context)?;
        if !is_valid_component_id(&id) {
            bail!("[remote-assets] {context}.id is invalid: {id}");
        }
        if !seen_ids.insert(id.clone()) {
            bail!("[remote-assets] duplicate component id in manifest: {id}");
        }
        let Some(expected_mount) = expected_mount(&id, expected_platform_arch) else {
            // 旧 manifest 可能仍含已退役组件；未知组件跳过，不阻断当前组件。
            continue;
        };

        let version = required_string(component, "version", &context)?;
        assert_safe_path_segment(&version, &format!("{context}.version"))?;
        let sha256 = required_string(component, "sha256", &context)?.to_lowercase();
        if sha256.len() != 64 || !sha256.chars().all(|value| value.is_ascii_hexdigit()) {
            bail!("[remote-assets] {context}.sha256 is invalid");
        }
        let artifact_path = normalize_relative_path(
            &required_string(component, "artifactPath", &context)?,
            &format!("{context}.artifactPath"),
        )?;
        let mount = normalize_relative_path(
            &required_string(component, "mount", &context)?,
            &format!("{context}.mount"),
        )?;
        if mount != expected_mount {
            bail!(
                "[remote-assets] {context}.mount mismatch for {id}: expected={expected_mount}, actual={mount}"
            );
        }
        components.push(ReleaseComponent {
            id,
            version,
            sha256,
            artifact_path,
            mount,
        });
    }

    Ok(ReleaseManifest {
        schema_version: u8::try_from(schema_version as u64)
            .map_err(|_| anyhow::anyhow!("[remote-assets] schemaVersion out of range"))?,
        app_version,
        platform_arch,
        components,
    })
}

/// 与 TS `normalizeRemoteAssetRelativePath` 一致：拒绝绝对路径、反斜杠与 `.`/`..` 段。
pub fn normalize_relative_path(raw_path: &str, label: &str) -> Result<String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        bail!("[remote-assets] {label} is empty");
    }
    if trimmed.contains('\\') {
        bail!("[remote-assets] {label} must not contain backslash: {raw_path}");
    }
    let bytes = trimmed.as_bytes();
    if trimmed.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        bail!("[remote-assets] {label} must be relative path: {raw_path}");
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        bail!("[remote-assets] {label} contains empty path segment: {raw_path}");
    }
    if segments.contains(&"..") || segments.contains(&".") {
        bail!("[remote-assets] {label} must not contain '.' or '..': {raw_path}");
    }
    Ok(segments.join("/"))
}

/// 与 TS `resolveRemoteAssetComponentCacheVersion` 一致：仅剥离 `+<12..64 hex>` 后缀。
pub fn component_cache_version(version: &str) -> &str {
    let Some((_, suffix)) = version.rsplit_once('+') else {
        return version;
    };
    if !(12..=64).contains(&suffix.len()) || !suffix.chars().all(|value| value.is_ascii_hexdigit())
    {
        return version;
    }
    &version[..version.len() - suffix.len() - 1]
}

/// 与 TS `readRemoteAssetComponentMeta` 一致：任意字段类型错误都当作「无 meta」。
pub fn parse_component_meta(content: &str) -> Option<ComponentMeta> {
    let value: Value = serde_json::from_str(content).ok()?;
    let map = value.as_object()?;
    let id = map.get("id")?.as_str()?.to_owned();
    let platform_arch = map.get("platformArch")?.as_str()?.to_owned();
    Some(ComponentMeta {
        id,
        version: optional_non_empty_string(map, "version")?,
        sha256: optional_non_empty_string(map, "sha256")?,
        pending_refresh_app_version: optional_non_empty_string(map, "pendingRefreshAppVersion")?,
        platform_arch,
    })
}

/// TS `shouldDeployVersionedComponent` 的 pure 版本；远端 `exists`/meta 读取由调用方注入。
pub fn should_deploy_component(options: DeployDecisionOptions<'_>) -> DeployDecision {
    if options.force {
        return DeployDecision::deploy("force deploy requested");
    }
    if !options.remote_exists {
        return DeployDecision::deploy(format!("remote file missing path={}", options.remote_path));
    }
    let Some(expected_raw) = options.expected_version else {
        return if options.fallback_deploy_when_version_unknown {
            DeployDecision::deploy("component version unavailable, using legacy full deploy")
        } else {
            DeployDecision::keep()
        };
    };
    let expected_version = component_cache_version(expected_raw);
    let Some(remote_meta) = options.remote_meta else {
        return DeployDecision::deploy(format!(
            "remote component meta missing expected={expected_version}"
        ));
    };
    if remote_meta.id != options.component_id {
        return DeployDecision::deploy(format!(
            "remote component id mismatch remote={} expected={}",
            remote_meta.id, options.component_id
        ));
    }
    if remote_meta.platform_arch != options.platform_arch {
        return DeployDecision::deploy(format!(
            "remote platform mismatch remote={} expected={}",
            remote_meta.platform_arch, options.platform_arch
        ));
    }
    let remote_version = remote_meta.version.as_deref().unwrap_or_default();
    if component_cache_version(remote_version) != component_cache_version(expected_version) {
        return DeployDecision::deploy(format!(
            "remote version mismatch remote={remote_version} expected={expected_version}"
        ));
    }
    DeployDecision::keep()
}

/// TS `checkRemoteAssetComponentIdentity`：内容寻址组件必须与 manifest SHA 一致。
pub fn check_component_identity(
    remote_meta: Option<&ComponentMeta>,
    component_id: &str,
    platform_arch: &str,
    expected_sha256: &str,
) -> DeployDecision {
    let Some(remote_meta) = remote_meta else {
        return DeployDecision::deploy(format!(
            "remote component meta missing expected={expected_sha256}"
        ));
    };
    if remote_meta.id != component_id {
        return DeployDecision::deploy(format!(
            "remote component id mismatch remote={} expected={component_id}",
            remote_meta.id
        ));
    }
    if remote_meta.platform_arch != platform_arch {
        return DeployDecision::deploy(format!(
            "remote platform mismatch remote={} expected={platform_arch}",
            remote_meta.platform_arch
        ));
    }
    let Some(remote_sha256) = remote_meta.sha256.as_deref() else {
        return DeployDecision::deploy(format!(
            "remote component SHA missing expected={expected_sha256}"
        ));
    };
    if remote_sha256.trim().to_lowercase() != expected_sha256.trim().to_lowercase() {
        return DeployDecision::deploy(format!(
            "remote SHA mismatch remote={remote_sha256} expected={expected_sha256}"
        ));
    }
    DeployDecision::keep()
}

/// 安装预览：版本取 manifest appVersion，安装根目录是 `~/.escode/server`。
pub fn build_install_preview(
    manifest: &ReleaseManifest,
    remote_home: Option<&str>,
    size_bytes: u64,
) -> Result<InstallPreview> {
    if size_bytes == 0 {
        bail!("install preview sizeBytes must be positive");
    }
    Ok(InstallPreview {
        version: manifest.app_version.clone(),
        install_path: remote_home.map(|home| resolve_posix_home_path(REMOTE_SERVER_ROOT, home)),
        size_bytes,
    })
}

fn required_string(map: &Map<String, Value>, key: &str, context: &str) -> Result<String> {
    let raw = map
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("[remote-assets] {context}.{key} must be string"))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("[remote-assets] {context}.{key} is empty");
    }
    Ok(trimmed.to_owned())
}

fn required_number(map: &Map<String, Value>, key: &str, context: &str) -> Result<f64> {
    map.get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow::anyhow!("[remote-assets] {context}.{key} must be finite number"))
}

fn optional_non_empty_string(map: &Map<String, Value>, key: &str) -> Option<Option<String>> {
    match map.get(key) {
        None => Some(None),
        Some(Value::String(value)) if value.is_empty() => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => None,
    }
}

fn is_valid_component_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(value) if value.is_ascii_lowercase() || value.is_ascii_digit())
        && chars.all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-')
}

fn assert_safe_path_segment(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("[remote-assets] {label} is empty");
    }
    if value.contains('/') || value.contains('\\') {
        bail!("[remote-assets] {label} must be a single path segment: {value}");
    }
    if value == "." || value == ".." {
        bail!("[remote-assets] {label} is invalid: {value}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "source_server_release_tests.rs"]
mod tests;
