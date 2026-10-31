//! 首次安装编排（S4-c，docs/specs/source-server-remote-rust-host.md §11.2/§11.3）。
//!
//! 连接前只做一件事：判断远端缺少哪些组件。显式连接补齐缺失组件后启动 server；
//! 已存在但版本/内容不同的组件不在连接流程里替换（更新只经 §11.4 的显式 updateSource）。
//! 安装本身复用 [`crate::source_server_install`]，本模块只负责探测、决策与两种来源的选择。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::source_server_archive::{ArchiveEntry, create_archive};
use crate::source_server_deploy_lock::AcquireOptions;
use crate::source_server_exec::RemoteRunner;
use crate::source_server_install::{
    ComponentInstall, InstallMode, InstallPlan, build_install_plan, install_plan_locked,
};
use crate::source_server_preflight::{
    PREFLIGHT_TIMEOUT, RemoteEnvironment, RemoteProbe, parse_environment, parse_tools, probe_script,
};
use crate::source_server_release::{
    ComponentMeta, InstallPreview, ReleaseManifest, build_install_preview, parse_component_meta,
    parse_release_manifest,
};
use crate::source_server_transport::quote_posix_path_arg;

/// Main 通过 `rpcListen` 交付的远程资源事实（Host 不读进程 env 推断路径）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteAssetsConfig {
    /// 本地 release 目录（含 `manifest-<platformArch>.json`，即 `releases/<version>`）。
    pub release_dir: Option<PathBuf>,
    /// CDN 基址（顺序尝试）；为空时只能本地上传。
    pub cdn_base_urls: Vec<String>,
    /// Main 的 app 版本；manifest 的 `appVersion` 必须与它一致。
    pub app_version: String,
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub assets: RemoteAssetsConfig,
    pub lock: AcquireOptions,
}

/// 已确定的安装计划：只包含远端缺失的组件。
#[derive(Clone, Debug)]
pub struct PreparedInstall {
    pub manifest: ReleaseManifest,
    pub plan: InstallPlan,
    pub mode: InstallMode,
    pub preview: Option<InstallPreview>,
}

/// 远端组件现状（`exists` 以组件首个落盘路径为准，meta 只用于诊断/预览）。
#[derive(Clone, Debug, Default)]
pub struct InspectedComponent {
    pub exists: bool,
    pub meta: Option<ComponentMeta>,
}

/// 探测远端并给出「需要安装什么」。全部组件已就位时返回 `None`。
pub async fn prepare_install(
    runner: &dyn RemoteRunner,
    assets: &RemoteAssetsConfig,
) -> Result<Option<PreparedInstall>> {
    let probe = probe_remote(runner).await?;
    let environment = &probe.environment;
    let manifest = match resolve_manifest(assets, &environment.platform_arch) {
        Ok(manifest) => manifest,
        // 没有本地 release 源时不能安装新版本；已部署的 server 继续连接（旧 S3 行为），
        // 只有必装组件也缺失时才把「无 release」作为显式失败上抛。
        Err(error) => {
            if required_components_present(runner).await {
                return Ok(None);
            }
            return Err(error);
        }
    };
    let full_plan = build_install_plan(&manifest).with_context(
        || "[source-server:install] remote-install-failed: invalid release manifest",
    )?;
    let inspected = inspect_remote(runner, &full_plan).await.with_context(
        || "[source-server:install] remote-install-failed: inspect remote components",
    )?;
    let missing: Vec<ComponentInstall> = full_plan
        .components
        .iter()
        .filter(|component| {
            !inspected
                .get(&component.component.id)
                .is_some_and(|state| state.exists)
        })
        .cloned()
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    let plan = InstallPlan {
        app_version: full_plan.app_version.clone(),
        platform_arch: full_plan.platform_arch.clone(),
        components: missing,
    };
    let mode = choose_mode(assets, &plan, &probe)?;
    let preview = build_preview(assets, &manifest, environment, &plan, &mode);
    Ok(Some(PreparedInstall {
        manifest,
        plan,
        mode,
        preview,
    }))
}

/// 获取部署锁并安装（锁覆盖全部组件；失败保留旧 release，只上抛显式错误）。
pub async fn apply_install(
    runner: &dyn RemoteRunner,
    prepared: &PreparedInstall,
    lock: &AcquireOptions,
) -> Result<()> {
    install_plan_locked(runner, &prepared.plan, &prepared.mode, lock)
        .await
        .map_err(|error| {
            let scope = match prepared.mode {
                InstallMode::LocalUpload { .. } => "upload",
                InstallMode::RemoteDownload { .. } => "download",
            };
            anyhow!(
                "[source-server:{scope}] {}: {error:#}",
                classify_install_failure(&error)
            )
        })
}

pub(crate) async fn probe_remote(runner: &dyn RemoteRunner) -> Result<RemoteProbe> {
    let output = runner
        .run(&probe_script(), None, PREFLIGHT_TIMEOUT)
        .await
        .map_err(|error| anyhow!("[source-server:preflight] remote-preflight-failed: {error:#}"))?;
    Ok(RemoteProbe {
        environment: parse_environment(&output.stdout).map_err(|error| {
            anyhow!("[source-server:preflight] remote-preflight-failed: {error:#}")
        })?,
        tools: parse_tools(&output.stdout).map_err(|error| {
            anyhow!("[source-server:preflight] remote-preflight-failed: {error:#}")
        })?,
    })
}

const REQUIRED_COMPONENTS_SCRIPT: &str = "if [ -e ~/.escode/server/node ] && [ -e ~/.escode/server/zcode-server.cjs ]; then printf 'present=1\\n'; else printf 'present=0\\n'; fi";

pub(crate) async fn required_components_present(runner: &dyn RemoteRunner) -> bool {
    runner
        .run(REQUIRED_COMPONENTS_SCRIPT, None, PREFLIGHT_TIMEOUT)
        .await
        .is_ok_and(|output| output.stdout.lines().any(|line| line.trim() == "present=1"))
}

pub(crate) fn resolve_manifest(
    assets: &RemoteAssetsConfig,
    platform_arch: &str,
) -> Result<ReleaseManifest> {
    let release_dir = assets.release_dir.as_deref().ok_or_else(|| {
        anyhow!(
            "[source-server:install] remote-install-failed: no local release directory for {platform_arch}; remote-download needs a CDN base URL"
        )
    })?;
    let path = release_dir.join(format!("manifest-{platform_arch}.json"));
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "[source-server:install] remote-install-failed: cannot read release manifest {}",
            path.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "[source-server:install] remote-install-failed: invalid release manifest {}",
            path.display()
        )
    })?;
    parse_release_manifest(&value, &assets.app_version, platform_arch).with_context(|| {
        format!(
            "[source-server:install] remote-install-failed: {}",
            path.display()
        )
    })
}

/// 一次往返读取组件是否存在与 component meta（不安装、不写远端）。
pub async fn inspect_remote(
    runner: &dyn RemoteRunner,
    plan: &InstallPlan,
) -> Result<BTreeMap<String, InspectedComponent>> {
    let output = runner
        .run(&inspect_script(plan), None, PREFLIGHT_TIMEOUT)
        .await
        .context("inspect remote components")?;
    parse_inspect(&output.stdout)
}

fn inspect_script(plan: &InstallPlan) -> String {
    let mut lines = Vec::new();
    for component in &plan.components {
        let id = &component.component.id;
        let Some(remote_path) = component
            .entries
            .first()
            .map(|entry| entry.remote_path.as_str())
        else {
            continue;
        };
        lines.push(format!(
            "if [ -e {path} ]; then printf '@@{id} exists=1\\n'; else printf '@@{id} exists=0\\n'; fi",
            path = quote_posix_path_arg(remote_path)
        ));
        let meta_path = quote_posix_path_arg(&ComponentMeta::path(id));
        lines.push(format!(
            "if [ -f {meta_path} ]; then printf '@@{id} meta=%s\\n' \"$(cat {meta_path} 2>/dev/null)\"; fi"
        ));
    }
    lines.join("\n")
}

fn parse_inspect(stdout: &str) -> Result<BTreeMap<String, InspectedComponent>> {
    let mut inspected: BTreeMap<String, InspectedComponent> = BTreeMap::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("@@") else {
            continue;
        };
        let Some((id, payload)) = rest.split_once(' ') else {
            continue;
        };
        let state = inspected.entry(id.to_owned()).or_default();
        if let Some(value) = payload.strip_prefix("exists=") {
            state.exists = value.trim() == "1";
        } else if let Some(value) = payload.strip_prefix("meta=") {
            state.meta = parse_component_meta(value);
        }
    }
    Ok(inspected)
}

pub(crate) fn choose_mode(
    assets: &RemoteAssetsConfig,
    plan: &InstallPlan,
    probe: &RemoteProbe,
) -> Result<InstallMode> {
    if let Some(release_dir) = assets.release_dir.as_deref()
        && local_artifacts_available(release_dir, plan)
    {
        return Ok(InstallMode::LocalUpload {
            release_dir: release_dir.to_path_buf(),
        });
    }
    if assets.cdn_base_urls.is_empty() {
        bail!(
            "[source-server:install] remote-install-failed: local release artifacts are incomplete and no CDN base URL is configured"
        );
    }
    Ok(InstallMode::RemoteDownload {
        base_urls: assets.cdn_base_urls.clone(),
        tools: probe.tools,
    })
}

fn local_artifacts_available(release_dir: &Path, plan: &InstallPlan) -> bool {
    plan.components.iter().all(|component| {
        component
            .entries
            .iter()
            .all(|entry| release_dir.join(&entry.source_relative_path).exists())
    })
}

/// 预览只报告可本地计算的大小：本地上传用归档字节数，远端下载用本地 artifact 大小；
/// 两者都不可计算时省略预览，不猜一个数字（schema 要求正数）。
pub(crate) fn build_preview(
    assets: &RemoteAssetsConfig,
    manifest: &ReleaseManifest,
    environment: &RemoteEnvironment,
    plan: &InstallPlan,
    mode: &InstallMode,
) -> Option<InstallPreview> {
    let size = match mode {
        InstallMode::LocalUpload { release_dir } => {
            plan.components.iter().try_fold(0_u64, |total, component| {
                let archive = local_archive(release_dir, component).ok()?;
                Some(total + archive.len() as u64)
            })?
        }
        InstallMode::RemoteDownload { .. } => {
            let release_dir = assets.release_dir.as_deref()?;
            plan.components.iter().try_fold(0_u64, |total, component| {
                let path = release_dir.join(&component.component.artifact_path);
                let size = std::fs::metadata(path).ok()?.len();
                Some(total + size)
            })?
        }
    };
    build_install_preview(manifest, environment.home.as_deref(), size).ok()
}

fn local_archive(release_dir: &Path, component: &ComponentInstall) -> Result<Vec<u8>> {
    let entries: Vec<ArchiveEntry> = component
        .entries
        .iter()
        .map(|entry| ArchiveEntry {
            source_path: release_dir.join(&entry.source_relative_path),
            archive_path: entry.source_relative_path.clone(),
        })
        .collect();
    create_archive(&entries)
}

/// 安装失败的错误码（spec §11.3 的显式集合）；不把传输层细节直接暴露给 UI。
pub fn classify_install_failure(error: &anyhow::Error) -> &'static str {
    let text = format!("{error:#}").to_lowercase();
    if text.contains("no space left") || text.contains("disk quota") {
        return "remote-space-insufficient";
    }
    if text.contains("permission denied") || text.contains("operation not permitted") {
        return "remote-permission-denied";
    }
    if text.contains("download or checksum failed") {
        return "remote-download-failed";
    }
    if text.contains("remote-upload-failed") || text.contains("cannot read local") {
        return "remote-upload-failed";
    }
    "remote-install-failed"
}

/// 从安装/探测错误里取出 spec §11.3 的显式错误码；无法归因时返回 None。
pub fn install_error_code(error: &anyhow::Error) -> Option<&'static str> {
    let text = format!("{error:#}");
    // 主机 key 变化是安全事件，不能被安装阶段码覆盖成 preflight/install 失败。
    if crate::source_server_transport::is_host_key_changed_error(&text) {
        return Some("remote-host-key-changed");
    }
    [
        "remote-preflight-failed",
        "remote-download-failed",
        "remote-upload-failed",
        "remote-install-failed",
        "remote-space-insufficient",
        "remote-permission-denied",
    ]
    .into_iter()
    .find(|code| text.contains(code))
}

#[cfg(test)]
#[path = "source_server_connections_identity_tests.rs"]
mod identity_tests;
#[cfg(test)]
#[path = "source_server_connections_install_tests.rs"]
mod install_tests;
#[cfg(test)]
#[path = "source_server_recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "source_server_bootstrap_test_support.rs"]
mod test_support;
#[cfg(test)]
#[path = "source_server_bootstrap_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "source_server_connections_update_tests.rs"]
mod update_tests;
