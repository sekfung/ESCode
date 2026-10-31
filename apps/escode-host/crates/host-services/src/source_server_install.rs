//! 首装/更新的组件安装执行（S4-c，docs/specs/source-server-remote-rust-host.md §11.3）。
//!
//! 部署单元是 component manifest（S4-b 解析）。两种来源在「远端临时目录 → 校验 → 解压 →
//! 原子替换」汇合：local-upload 把 release 目录打成 tar.gz 经 stdin 上传；remote-download
//! 由远端 curl/wget 下载 artifact 并校验 manifest sha256。安装成功后才写 component meta。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::source_server_archive::{ArchiveEntry, create_archive};
use crate::source_server_deploy_lock::{AcquireOptions, acquire};
use crate::source_server_exec::RemoteRunner;
use crate::source_server_preflight::{DownloadTool, RemoteAssetTools, Sha256Tool};
use crate::source_server_release::{
    ComponentMeta, REMOTE_SERVER_ROOT, ReleaseComponent, ReleaseManifest,
};
use crate::source_server_transport::{quote_posix_path_arg, quote_posix_shell_arg};

pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallEntryKind {
    File { executable: bool },
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallEntry {
    pub kind: InstallEntryKind,
    pub source_relative_path: String,
    pub remote_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    pub remote_path: String,
    pub content: String,
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentInstall {
    pub component: ReleaseComponent,
    pub platform_arch: String,
    pub entries: Vec<InstallEntry>,
    pub generated: Vec<GeneratedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    pub app_version: String,
    pub platform_arch: String,
    pub components: Vec<ComponentInstall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMode {
    LocalUpload {
        release_dir: PathBuf,
    },
    RemoteDownload {
        base_urls: Vec<String>,
        tools: RemoteAssetTools,
    },
}

/// 固定安装顺序：先 server 运行时，再随包工具；未知/缺失的可选组件跳过。
pub fn build_install_plan(manifest: &ReleaseManifest) -> Result<InstallPlan> {
    let mut components = Vec::new();
    for id in [
        "node-runtime",
        "server-bundle",
        "node-pty",
        "glm",
        "bfs",
        "ripgrep",
        "ugrep",
    ] {
        let Some(component) = manifest.components.iter().find(|value| value.id == id) else {
            if matches!(id, "node-runtime" | "server-bundle") {
                bail!("manifest missing required component {id}");
            }
            continue;
        };
        components.push(component_install(component, &manifest.platform_arch));
    }
    Ok(InstallPlan {
        app_version: manifest.app_version.clone(),
        platform_arch: manifest.platform_arch.clone(),
        components,
    })
}

pub async fn install_plan(
    runner: &dyn RemoteRunner,
    plan: &InstallPlan,
    mode: &InstallMode,
) -> Result<()> {
    for component in &plan.components {
        match mode {
            InstallMode::LocalUpload { release_dir } => {
                let archive = local_archive(release_dir, component)?;
                let script = local_install_script(component)?;
                runner.run(&script, Some(&archive), INSTALL_TIMEOUT).await?;
            }
            InstallMode::RemoteDownload { base_urls, tools } => {
                let script = remote_install_script(component, base_urls, *tools)?;
                runner.run(&script, None, INSTALL_TIMEOUT).await?;
            }
        }
    }
    Ok(())
}

/// 安装事务：锁覆盖全部组件；失败时仍释放锁，并把安装错误优先返回。
pub async fn install_plan_locked(
    runner: &dyn RemoteRunner,
    plan: &InstallPlan,
    mode: &InstallMode,
    lock: &AcquireOptions,
) -> Result<()> {
    let lock = acquire(runner, lock).await?;
    let outcome = install_plan(runner, plan, mode).await;
    let release = lock.release().await;
    match (outcome, release) {
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

fn component_install(component: &ReleaseComponent, platform: &str) -> ComponentInstall {
    let base = REMOTE_SERVER_ROOT;
    let version = display_version(component);
    let (entries, generated) = match component.id.as_str() {
        "node-runtime" => (
            vec![file(
                format!("node/{platform}/node"),
                format!("{base}/node"),
                true,
            )],
            Vec::new(),
        ),
        "server-bundle" => (
            vec![file(
                "server/zcode-server.cjs".to_owned(),
                format!("{base}/zcode-server.cjs"),
                false,
            )],
            Vec::new(),
        ),
        "node-pty" => {
            let mut entries = vec![file(
                format!("node-pty/{platform}/pty.node"),
                format!("{base}/build/Release/pty.node"),
                false,
            )];
            if platform.starts_with("darwin-") {
                entries.push(file(
                    format!("node-pty/{platform}/spawn-helper"),
                    format!("{base}/build/Release/spawn-helper"),
                    true,
                ));
            }
            (entries, Vec::new())
        }
        "glm" => (
            vec![directory(
                format!("glm/{platform}"),
                format!("{base}/agents/glm"),
            )],
            vec![
                generated_file(
                    format!("{base}/agents/glm/escode-agent"),
                    agent_wrapper("glm"),
                    true,
                ),
                generated_file(
                    format!("{base}/agents/glm/.version"),
                    format!("{version}\n"),
                    false,
                ),
            ],
        ),
        id @ ("bfs" | "ripgrep" | "ugrep") => {
            let binary = match id {
                "bfs" => "bfs",
                "ripgrep" => "rg",
                _ => "ugrep",
            };
            (
                vec![file(
                    format!("tools/{platform}/{id}/{binary}"),
                    format!("{base}/tools/{id}/{binary}"),
                    true,
                )],
                vec![generated_file(
                    format!("{base}/tools/{id}/.version"),
                    format!("{version}\n"),
                    false,
                )],
            )
        }
        _ => (Vec::new(), Vec::new()),
    };
    ComponentInstall {
        component: component.clone(),
        platform_arch: platform.to_owned(),
        entries,
        generated,
    }
}

fn display_version(component: &ReleaseComponent) -> String {
    component.cache_version().trim_start_matches('v').to_owned()
}

fn agent_wrapper(provider: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\nruntime_root=\"${{ESCODE_SERVER_RUNTIME_ROOT:-$HOME/.escode/server}}\"\nexec \"$runtime_root/node\" \"$HOME/.escode/server/agents/{provider}/zcode.cjs\" \"$@\"\n"
    )
}

fn file(source_relative_path: String, remote_path: String, executable: bool) -> InstallEntry {
    InstallEntry {
        kind: InstallEntryKind::File { executable },
        source_relative_path,
        remote_path,
    }
}

fn directory(source_relative_path: String, remote_path: String) -> InstallEntry {
    InstallEntry {
        kind: InstallEntryKind::Directory,
        source_relative_path,
        remote_path,
    }
}

fn generated_file(remote_path: String, content: String, executable: bool) -> GeneratedFile {
    GeneratedFile {
        remote_path,
        content,
        executable,
    }
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

fn local_install_script(component: &ComponentInstall) -> Result<String> {
    let staging = format!("{REMOTE_SERVER_ROOT}/.install-{}", uuid::Uuid::new_v4());
    let mut lines = vec![
        "set -eu".to_owned(),
        format!("staging={}", quote_posix_path_arg(&staging)),
        "rm -rf \"$staging\"".to_owned(),
        "mkdir -p \"$staging\"".to_owned(),
        "tar -xzf - -C \"$staging\"".to_owned(),
    ];
    lines.extend(install_actions(component, ArchiveLayout::ReleaseTree)?);
    lines.push("rm -rf \"$staging\"".to_owned());
    Ok(lines.join("\n"))
}

fn remote_install_script(
    component: &ComponentInstall,
    base_urls: &[String],
    tools: RemoteAssetTools,
) -> Result<String> {
    let urls: Vec<String> = base_urls
        .iter()
        .map(|base| base.trim().trim_end_matches('/'))
        .filter(|base| !base.is_empty())
        .map(|base| format!("{base}/{}", component.component.artifact_path))
        .collect();
    if urls.is_empty() {
        bail!("remote-download install needs at least one CDN base URL");
    }
    let token = uuid::Uuid::new_v4();
    let artifact = quote_posix_path_arg(&format!("{REMOTE_SERVER_ROOT}/.install-{token}.tar.gz"));
    let staging = quote_posix_path_arg(&format!("{REMOTE_SERVER_ROOT}/.install-{token}"));
    let download = match tools.download {
        DownloadTool::Curl => {
            "curl -fL --retry 2 --connect-timeout 20 -o \"$artifact\" \"$url\"".to_owned()
        }
        DownloadTool::Wget => "wget --tries=3 --timeout=20 -O \"$artifact\" \"$url\"".to_owned(),
    };
    let checksum = checksum_command(tools.sha256);
    let mut lines = vec![
        "set -eu".to_owned(),
        format!("artifact={artifact}"),
        format!("staging={staging}"),
        "rm -rf \"$staging\" \"$artifact\"".to_owned(),
        "mkdir -p \"$staging\"".to_owned(),
        "downloaded=0".to_owned(),
        format!(
            "for url in {}; do",
            urls.iter()
                .map(|url| quote_posix_shell_arg(url))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "  rm -f \"$artifact\"".to_owned(),
        format!("  if {download}; then"),
        format!("    actual_sha=$({checksum})"),
        format!(
            "    if [ \"$actual_sha\" = {} ]; then downloaded=1; break; fi",
            quote_posix_shell_arg(&component.component.sha256)
        ),
        "  fi".to_owned(),
        "done".to_owned(),
        "if [ \"$downloaded\" != 1 ]; then echo 'remote artifact download or checksum failed' >&2; rm -rf \"$staging\" \"$artifact\"; exit 1; fi".to_owned(),
        "tar -xzf \"$artifact\" -C \"$staging\"".to_owned(),
    ];
    // 发布的 CDN 制品以 component mount 为归档根（见 mock-cdn/components/<platform>/…），
    // 与 local-upload 复用 release 目录树不同；安装路径必须相对 mount 还原。
    lines.extend(install_actions(component, ArchiveLayout::ComponentMount)?);
    lines.push("rm -rf \"$staging\" \"$artifact\"".to_owned());
    Ok(lines.join("\n"))
}

fn checksum_command(tool: Sha256Tool) -> String {
    match tool {
        Sha256Tool::Sha256sum => "sha256sum \"$artifact\" | awk '{print $1}'".to_owned(),
        Sha256Tool::Shasum => "shasum -a 256 \"$artifact\" | awk '{print $1}'".to_owned(),
        Sha256Tool::Openssl => "openssl dgst -sha256 \"$artifact\" | awk '{print $NF}'".to_owned(),
    }
}

#[derive(Clone, Copy)]
enum ArchiveLayout {
    /// local-upload：归档保留 release 目录树（`node/<platform>/node`、`server/zcode-server.cjs`）。
    ReleaseTree,
    /// remote-download：制品归档根就是 component mount 的内容。
    ComponentMount,
}

fn install_actions(component: &ComponentInstall, layout: ArchiveLayout) -> Result<Vec<String>> {
    let mut lines = vec![
        "install_file() { src=\"$1\"; dst=\"$2\"; exe=\"$3\"; mkdir -p \"$(dirname \"$dst\")\"; rm -f \"$dst\"; cp -p \"$src\" \"$dst\"; if [ \"$exe\" = 1 ]; then chmod +x \"$dst\"; fi; }".to_owned(),
        "install_dir() { src=\"$1\"; dst=\"$2\"; mkdir -p \"$(dirname \"$dst\")\"; rm -rf \"$dst.new\"; cp -a \"$src\" \"$dst.new\"; rm -rf \"$dst.old\"; if [ -e \"$dst\" ]; then mv \"$dst\" \"$dst.old\"; fi; mv \"$dst.new\" \"$dst\"; rm -rf \"$dst.old\"; }".to_owned(),
        "write_file() { dst=\"$1\"; content=\"$2\"; exe=\"$3\"; mkdir -p \"$(dirname \"$dst\")\"; printf %s \"$content\" > \"$dst\"; if [ \"$exe\" = 1 ]; then chmod +x \"$dst\"; fi; }".to_owned(),
    ];
    for entry in &component.entries {
        let source_relative_path = match layout {
            ArchiveLayout::ReleaseTree => entry.source_relative_path.clone(),
            ArchiveLayout::ComponentMount => {
                strip_component_mount(&component.component.mount, &entry.source_relative_path)
            }
        };
        let source = format!(
            "\"$staging\"/{}",
            quote_posix_shell_arg(&source_relative_path)
        );
        let remote = quote_posix_path_arg(&entry.remote_path);
        match entry.kind {
            InstallEntryKind::File { executable } => {
                lines.push(format!(
                    "install_file {source} {remote} {}",
                    if executable { 1 } else { 0 }
                ));
            }
            InstallEntryKind::Directory => lines.push(format!("install_dir {source} {remote}")),
        }
    }
    for generated in &component.generated {
        lines.push(format!(
            "write_file {} {} {}",
            quote_posix_path_arg(&generated.remote_path),
            quote_posix_shell_arg(&generated.content),
            if generated.executable { 1 } else { 0 }
        ));
    }
    let meta = ComponentMeta {
        id: component.component.id.clone(),
        version: Some(component.component.version.clone()),
        sha256: Some(component.component.sha256.clone()),
        pending_refresh_app_version: None,
        platform_arch: component.platform_arch.clone(),
    };
    lines.push(format!(
        "write_file {} {} 0",
        quote_posix_path_arg(&ComponentMeta::path(&meta.id)),
        quote_posix_shell_arg(&meta.to_json_line()?)
    ));
    Ok(lines)
}

/// 与 TS `buildRemoteComponentSourcePath` 同义：归档内路径相对 mount。
fn strip_component_mount(mount: &str, source_relative_path: &str) -> String {
    let mount = mount.trim_end_matches('/');
    let source = source_relative_path.trim_matches('/');
    if source == mount {
        return ".".to_owned();
    }
    source
        .strip_prefix(&format!("{mount}/"))
        .unwrap_or(source)
        .to_owned()
}

#[cfg(test)]
#[path = "source_server_install_tests.rs"]
mod tests;
