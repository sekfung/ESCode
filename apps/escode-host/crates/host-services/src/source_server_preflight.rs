//! 远端平台探测与安装前置检查（S4-b，docs/specs/source-server-remote-rust-host.md §11.3）。
//!
//! 与 TS `detectEnv.ts` / `remoteAssetPreflight.ts` 对齐：一次 POSIX shell 往返同时探测
//! `uname -s/-m`、`/proc` 内核类型、`$HOME` 与 `curl|wget`、`tar`、`sha256sum|shasum|openssl`。
//! 本切片只读取事实，不安装、不写远端；失败按显式错误返回，不静默降级。

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::source_server_exec::{RemoteExecutor, RemoteRunner};
use crate::source_server_transport::{TransportSecrets, TransportTarget};

pub const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadTool {
    Curl,
    Wget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sha256Tool {
    Sha256sum,
    Shasum,
    Openssl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteAssetTools {
    pub download: DownloadTool,
    pub tar: bool,
    pub sha256: Sha256Tool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEnvironment {
    pub platform: String,
    pub arch: String,
    /// `platform-arch`，即 manifest 文件名与组件路径使用的标识。
    pub platform_arch: String,
    pub home: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteProbe {
    pub environment: RemoteEnvironment,
    pub tools: RemoteAssetTools,
}

pub fn probe_script() -> String {
    format!("{}; {}", environment_script(), preflight_script())
}

fn environment_script() -> &'static str {
    concat!(
        "printf 'platform=%s\\n' \"$(uname -s)\"; ",
        "printf 'arch=%s\\n' \"$(uname -m)\"; ",
        "printf 'kernel_ostype=%s\\n' \"$(cat /proc/sys/kernel/ostype 2>/dev/null)\"; ",
        "printf 'home=%s\\n' \"$HOME\""
    )
}

/// 与 TS `buildPreflightCommand` 的探测顺序一致（curl 优先、sha256sum 优先）。
fn preflight_script() -> &'static str {
    concat!(
        "download=; ",
        "if command -v curl >/dev/null 2>&1; then download=curl; ",
        "elif command -v wget >/dev/null 2>&1; then download=wget; fi; ",
        "tar_tool=; ",
        "if command -v tar >/dev/null 2>&1; then tar_tool=tar; fi; ",
        "sha_tool=; ",
        "if command -v sha256sum >/dev/null 2>&1; then sha_tool=sha256sum; ",
        "elif command -v shasum >/dev/null 2>&1; then sha_tool=shasum; ",
        "elif command -v openssl >/dev/null 2>&1; then sha_tool=openssl; fi; ",
        "printf 'download=%s\\ntar=%s\\nsha256=%s\\n' \"$download\" \"$tar_tool\" \"$sha_tool\""
    )
}

pub fn parse_probe(stdout: &str) -> Result<RemoteProbe> {
    Ok(RemoteProbe {
        environment: parse_environment(stdout)?,
        tools: parse_tools(stdout)?,
    })
}

/// 只解析平台/架构/`$HOME`；`local-download-upload` 不依赖远端下载工具，不应因缺少
/// `curl`/`sha256sum` 被拒绝（spec §11.3 的 preflight 只属于 `remote-download`）。
pub fn parse_environment(stdout: &str) -> Result<RemoteEnvironment> {
    let values = parse_key_values(stdout);
    let reported = required_value(&values, "platform")?;
    let arch = required_value(&values, "arch")?;
    let kernel_ostype = values.get("kernel_ostype").map_or("", String::as_str);
    let platform = resolve_remote_platform(reported, kernel_ostype);
    let arch = normalize_remote_arch(arch);
    let home = values
        .get("home")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(RemoteEnvironment {
        platform_arch: format!("{platform}-{arch}"),
        platform,
        arch,
        home,
    })
}

pub fn parse_tools(stdout: &str) -> Result<RemoteAssetTools> {
    let values = parse_key_values(stdout);
    let download = match values.get("download").map(String::as_str) {
        Some("curl") => DownloadTool::Curl,
        Some("wget") => DownloadTool::Wget,
        _ => bail!("remote-preflight-failed: missing curl or wget"),
    };
    if values.get("tar").map(String::as_str) != Some("tar") {
        bail!("remote-preflight-failed: missing tar");
    }
    let sha256 = match values.get("sha256").map(String::as_str) {
        Some("sha256sum") => Sha256Tool::Sha256sum,
        Some("shasum") => Sha256Tool::Shasum,
        Some("openssl") => Sha256Tool::Openssl,
        _ => bail!("remote-preflight-failed: missing sha256sum, shasum or openssl"),
    };
    Ok(RemoteAssetTools {
        download,
        tar: true,
        sha256,
    })
}

/// 与 TS `normalizeRemotePlatform` 一致。
pub fn normalize_remote_platform(raw_platform: &str) -> String {
    let platform = raw_platform.trim().to_lowercase();
    if platform == "darwin" || platform == "macos" {
        return "darwin".to_owned();
    }
    if platform == "linux" || platform == "gnu/linux" {
        return "linux".to_owned();
    }
    if platform == "windows_nt"
        || platform.starts_with("mingw")
        || platform.starts_with("msys")
        || platform.starts_with("cygwin")
    {
        return "win32".to_owned();
    }
    platform
}

/// 与 TS `normalizeRemoteArch` 一致。
pub fn normalize_remote_arch(raw_arch: &str) -> String {
    match raw_arch.trim().to_lowercase().as_str() {
        "x86_64" | "amd64" => "x64".to_owned(),
        "aarch64" | "arm64e" => "arm64".to_owned(),
        arch => arch.to_owned(),
    }
}

/// 某些 SSH/Docker 容器把 `uname -s` 伪装成 Darwin，但 `/proc` 仍是 Linux 内核。
pub fn resolve_remote_platform(reported_platform: &str, kernel_ostype: &str) -> String {
    let reported = normalize_remote_platform(reported_platform);
    let kernel = normalize_remote_platform(kernel_ostype);
    if reported == "darwin" && kernel == "linux" {
        return "linux".to_owned();
    }
    reported
}

/// 经 transport 执行组合探测脚本；transport 差异不进入解析逻辑。
pub async fn probe_over_transport(
    target: &TransportTarget,
    secrets: &TransportSecrets,
    askpass_program: &Path,
    known_hosts: Option<&Path>,
    timeout: Duration,
) -> Result<RemoteProbe> {
    let executor = RemoteExecutor::new(target.clone(), secrets.clone(), askpass_program)
        .with_known_hosts(known_hosts.map(Path::to_path_buf));
    let output = executor.run(&probe_script(), None, timeout).await?;
    parse_probe(&output.stdout)
}

fn parse_key_values(stdout: &str) -> std::collections::BTreeMap<String, String> {
    let mut values = std::collections::BTreeMap::new();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        values.insert(key.to_owned(), value.trim().to_owned());
    }
    values
}

fn required_value<'a>(
    values: &'a std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<&'a str> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("remote probe missing {key}"))
}

#[cfg(test)]
#[path = "source_server_preflight_tests.rs"]
mod tests;
