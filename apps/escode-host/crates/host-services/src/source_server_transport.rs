//! Source Server transport 启动命令（S3，docs/specs/source-server-remote-rust-host.md §10.3）。
//!
//! 只把 Connection Profile 变成一条可执行的 stdio 命令，不建立连接、不做部署：
//! SSH 用系统 OpenSSH（与 `scripts/verify-remote-ssh.mjs`、`sshConfigAlias` 一致），
//! WSL/Docker 用 `sh -lc`。远端命令与参数在链路之上完全一致，transport 差异不进入业务域。
//!
//! host-key 策略（S4-h）：首次连接 `accept-new` 写入 Desktop 自有 known_hosts；已存在但 key
//! 变化时 OpenSSH 拒绝连接，调用方把 stderr 映射为 `remote-host-key-changed`，不自动覆盖。
//!
//! 密码/口令只经子进程 env 传给自带 `--ssh-askpass` 模式的 Host 可执行文件，不进入 argv。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::source_server::ConnectionProfile;

/// 解析后的 transport 目标（只包含启动命令需要的字段）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportTarget {
    Ssh(SshTarget),
    Wsl(WslTarget),
    Docker(DockerTarget),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub private_key_path: Option<PathBuf>,
    pub password_credential_key: Option<String>,
    pub private_key_passphrase_credential_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslTarget {
    pub distro: Option<String>,
    pub user: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockerTarget {
    pub container: String,
}

/// 已从 credential 服务解出的 secret；`None` 表示 profile 没要求。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransportSecrets {
    pub password: Option<String>,
    pub private_key_passphrase: Option<String>,
}

/// 一条可直接交给 `Link::Process` 的启动命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
}

pub struct BuildOptions<'a> {
    /// Host 可执行文件（`--ssh-askpass` 模式）；SSH 使用密码/口令时必需。
    pub askpass_program: &'a Path,
    /// 远端 server / runtime 命令（已按远端平台语义拆成 argv）。
    pub remote_command: &'a [String],
    /// SSH 的 Desktop 自有 known_hosts；WSL/Docker 不使用。SSH 缺省时显式失败。
    pub known_hosts: Option<&'a Path>,
}

/// Desktop 自有 known_hosts：`<dataDir>/.escode/v2/known_hosts`（spec §11.8）。
pub fn default_known_hosts_path(data_base_dir: &Path) -> PathBuf {
    data_base_dir.join(".escode").join("v2").join("known_hosts")
}

/// 从 settings 里的 Connection Profile 解析 transport 目标。未知/缺字段显式报错。
pub fn parse_target(profile: &ConnectionProfile) -> Result<TransportTarget> {
    match profile.transport.as_str() {
        "ssh" => {
            let target = &profile.target;
            Ok(TransportTarget::Ssh(SshTarget {
                host: required_string(target, "host")?,
                port: optional_port(target)?,
                username: required_string(target, "username")?,
                private_key_path: optional_path(target, "privateKeyPath"),
                password_credential_key: optional_string(target, "passwordCredentialKey"),
                private_key_passphrase_credential_key: optional_string(
                    target,
                    "privateKeyPassphraseCredentialKey",
                ),
            }))
        }
        "wsl" => Ok(TransportTarget::Wsl(WslTarget {
            distro: optional_string(&profile.target, "distro"),
            user: optional_string(&profile.target, "user"),
        })),
        "docker" => Ok(TransportTarget::Docker(DockerTarget {
            container: required_string(&profile.target, "container")?,
        })),
        other => bail!("unsupported source server transport: {other}"),
    }
}

/// 构造启动命令。密码/口令缺失（profile 要求但未解出）时显式失败，不回退到 agent 认证。
pub fn build_transport_command(
    target: &TransportTarget,
    secrets: &TransportSecrets,
    options: &BuildOptions<'_>,
) -> Result<TransportCommand> {
    match target {
        TransportTarget::Ssh(ssh) => build_ssh(ssh, secrets, options),
        TransportTarget::Wsl(wsl) => {
            let mut args: Vec<OsString> = Vec::new();
            if let Some(distro) = wsl.distro.as_deref().filter(|value| !value.is_empty()) {
                args.push("-d".into());
                args.push(distro.into());
            }
            if let Some(user) = wsl.user.as_deref().filter(|value| !value.is_empty()) {
                args.push("-u".into());
                args.push(user.into());
            }
            args.extend(["--".into(), "sh".into(), "-lc".into()]);
            args.push(join_quoted(options.remote_command).into());
            Ok(TransportCommand {
                program: PathBuf::from("wsl.exe"),
                args,
                env: BTreeMap::new(),
            })
        }
        TransportTarget::Docker(docker) => Ok(TransportCommand {
            program: PathBuf::from("docker"),
            args: vec![
                "exec".into(),
                "-i".into(),
                docker.container.clone().into(),
                "sh".into(),
                "-lc".into(),
                join_quoted(options.remote_command).into(),
            ],
            env: BTreeMap::new(),
        }),
    }
}

fn build_ssh(
    ssh: &SshTarget,
    secrets: &TransportSecrets,
    options: &BuildOptions<'_>,
) -> Result<TransportCommand> {
    let secret = resolved_secret(ssh, secrets)?;
    let mut args: Vec<OsString> = vec!["-T".into()];
    args.extend([
        "-o".into(),
        if secret.is_some() {
            "BatchMode=no".into()
        } else {
            "BatchMode=yes".into()
        },
    ]);
    let Some(known_hosts) = options.known_hosts else {
        bail!("ssh transport requires a Desktop-owned known_hosts path");
    };
    for option in [
        "StrictHostKeyChecking=accept-new".to_owned(),
        format!("UserKnownHostsFile={}", known_hosts.to_string_lossy()),
        "LogLevel=ERROR".to_owned(),
        "ConnectTimeout=15".to_owned(),
        "ServerAliveInterval=15".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
    ] {
        args.push("-o".into());
        args.push(option.into());
    }
    if let Some(key) = &ssh.private_key_path {
        args.push("-i".into());
        args.push(key.clone().into_os_string());
    }
    args.push("-p".into());
    args.push(ssh.port.to_string().into());
    args.push("--".into());
    args.push(format!("{}@{}", ssh.username, ssh.host).into());
    args.push(join_quoted(options.remote_command).into());

    let mut env = BTreeMap::new();
    if let Some(secret) = secret {
        env.insert("ESCODE_SSH_ASKPASS_SECRET".to_owned(), secret.to_owned());
        env.insert(
            "SSH_ASKPASS".to_owned(),
            options.askpass_program.to_string_lossy().into_owned(),
        );
        env.insert("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned());
        env.insert("DISPLAY".to_owned(), ":0".to_owned());
    }
    Ok(TransportCommand {
        program: PathBuf::from("ssh"),
        args,
        env,
    })
}

/// OpenSSH 对「key 已变化」的明确诊断；不复用普通认证/握手失败猜测身份变化。
pub fn is_host_key_changed_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("remote host identification has changed")
        || message.contains("host key verification failed")
}

fn resolved_secret<'a>(ssh: &SshTarget, secrets: &'a TransportSecrets) -> Result<Option<&'a str>> {
    match &ssh.private_key_passphrase_credential_key {
        Some(key) => match secrets.private_key_passphrase.as_deref() {
            Some(secret) => Ok(Some(secret)),
            None => bail!("private key passphrase not resolved for credential {key}"),
        },
        None => match &ssh.password_credential_key {
            Some(key) => match secrets.password.as_deref() {
                Some(secret) => Ok(Some(secret)),
                None => bail!("password not resolved for credential {key}"),
            },
            None => Ok(None),
        },
    }
}

/// 与 TS `quotePosixShellArg` 逐字节一致：始终单引号，内部 `'` 用 `'"'"'` 收口。
pub fn quote_posix_shell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

/// 与 TS `buildPosixShellExecCommand` 一致。
pub fn build_posix_shell_exec_command(command: &str) -> String {
    format!("/bin/sh -c {}", quote_posix_shell_arg(command))
}

/// 与 TS `quotePosixPathArg` 一致：`~` / `~/x` 展开为 `$HOME`。
pub fn quote_posix_path_arg(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".to_owned();
    }
    if let Some(rest) = path.strip_prefix('~').filter(|rest| rest.starts_with('/')) {
        return format!("\"$HOME\"{}", quote_posix_shell_arg(rest));
    }
    quote_posix_shell_arg(path)
}

/// 与 TS `resolvePosixHomePath` 一致。
pub fn resolve_posix_home_path(remote_path: &str, home_dir: &str) -> String {
    if remote_path == "~" {
        return home_dir.to_owned();
    }
    match remote_path.strip_prefix("~/") {
        Some(rest) => format!("{}/{}", home_dir.trim_end_matches('/'), rest),
        None => remote_path.to_owned(),
    }
}

fn join_quoted(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| quote_posix_shell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn required_string(target: &Value, key: &str) -> Result<String> {
    match target.get(key).and_then(Value::as_str).map(str::trim) {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => bail!("profile target missing {key}"),
    }
}

fn optional_string(target: &Value, key: &str) -> Option<String> {
    target
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn optional_path(target: &Value, key: &str) -> Option<PathBuf> {
    optional_string(target, key).map(PathBuf::from)
}

fn optional_port(target: &Value) -> Result<u16> {
    match target.get("port") {
        None | Some(Value::Null) => Ok(22),
        Some(value) => {
            let port = value
                .as_u64()
                .filter(|port| (1..=65_535).contains(port))
                .ok_or_else(|| anyhow::anyhow!("profile target port out of range"))?;
            Ok(port as u16)
        }
    }
}

#[cfg(test)]
#[path = "source_server_transport_tests.rs"]
mod tests;
