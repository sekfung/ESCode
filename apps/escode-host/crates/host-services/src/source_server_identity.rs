//! Source Server 持久身份 probe（S4-j，docs/specs/source-server-remote-rust-host.md §11.12）。
//!
//! 远端 `~/.escode/server/install.json` 是 `sourceServerId` 的唯一来源；缺失时由 Host 生成 UUID，
//! 经临时文件 + hard link 原子发布，避免并发连接各自写入不同身份。probe 同时返回 `osUser`、
//! `hostname` 与默认 workspace，不读 Profile 或路径推断。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::source_server_connection_state::now_ms;
use crate::source_server_exec::{CommandOutput, RemoteRunner};
use crate::source_server_transport::quote_posix_shell_arg;

/// probe 的上限；远端无响应时显式失败，不把未知身份当成空值。
pub const IDENTITY_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerIdentity {
    pub source_server_id: String,
    pub os_user: Option<String>,
    pub hostname: Option<String>,
    pub workspace_path: Option<String>,
}

/// 读取远端持久身份；`install.json` 不存在时先原子创建。
pub async fn probe_server_identity(
    runner: Arc<dyn RemoteRunner>,
    timeout: Duration,
) -> Result<ServerIdentity> {
    let output = runner
        .run(&read_script(), None, timeout)
        .await
        .context("probe source server identity")?;
    let probed = parse_read_output(&output)?;
    let marker = match probed.marker {
        Marker::Present(marker) => marker,
        Marker::Missing { canonical_root } => {
            write_install_marker(runner, timeout, &canonical_root).await?
        }
    };
    identity_from_marker(
        &marker,
        probed.os_user,
        probed.hostname,
        probed.workspace_path,
    )
}

fn read_script() -> String {
    r#"set -eu
root="$HOME/.escode/server"
mkdir -p "$root"
marker="$root/install.json"
if [ -f "$marker" ]; then
  printf "present\0%s\0" "$(cat "$marker")"
else
  printf "missing\0%s\0" "$(cd "$root" && pwd -P)"
fi
user=$(id -un 2>/dev/null || true)
host=$(hostname 2>/dev/null || true)
workspace="${ESCODE_SERVER_WORKSPACE:-$(pwd)}"
printf "%s\0%s\0%s\0" "$user" "$host" "$workspace"
"#
    .to_owned()
}

async fn write_install_marker(
    runner: Arc<dyn RemoteRunner>,
    timeout: Duration,
    canonical_root: &str,
) -> Result<String> {
    let marker = json!({
        "product": "escode-server",
        "schemaVersion": 1,
        "canonicalServerRoot": canonical_root,
        "installationId": uuid::Uuid::new_v4().to_string(),
        "installedAt": now_ms(),
    });
    let payload = quote_posix_shell_arg(&serde_json::to_string(&marker)?);
    let script = format!(
        r#"set -eu
root="$HOME/.escode/server"
marker="$root/install.json"
mkdir -p "$root/run"
tmp="$root/run/.install-$$-${{RANDOM:-0}}.tmp"
printf "%s\n" {payload} > "$tmp"
if ln "$tmp" "$marker" 2>/dev/null; then :; fi
rm -f "$tmp"
cat "$marker"
"#
    );
    let output = runner
        .run(&script, None, timeout)
        .await
        .context("create source server install marker")?;
    Ok(output.stdout)
}

enum Marker {
    Present(String),
    Missing { canonical_root: String },
}

struct ReadOutput {
    marker: Marker,
    os_user: Option<String>,
    hostname: Option<String>,
    workspace_path: Option<String>,
}

fn parse_read_output(output: &CommandOutput) -> Result<ReadOutput> {
    let mut parts = output.stdout.split_terminator('\0');
    let status = parts.next().context("identity probe returned no status")?;
    let marker_or_root = parts
        .next()
        .context("identity probe returned no install marker")?;
    let os_user = parts.next().context("identity probe returned no osUser")?;
    let hostname = parts
        .next()
        .context("identity probe returned no hostname")?;
    let workspace_path = parts
        .next()
        .context("identity probe returned no workspace")?;
    if parts.next().is_some() {
        bail!("identity probe returned unexpected trailing fields");
    }
    let marker = match status {
        "present" => Marker::Present(marker_or_root.to_owned()),
        "missing" => Marker::Missing {
            canonical_root: marker_or_root.to_owned(),
        },
        other => bail!("identity probe returned unknown status: {other}"),
    };
    Ok(ReadOutput {
        marker,
        os_user: non_empty(os_user),
        hostname: non_empty(hostname),
        workspace_path: non_empty(workspace_path),
    })
}

fn identity_from_marker(
    marker: &str,
    os_user: Option<String>,
    hostname: Option<String>,
    workspace_path: Option<String>,
) -> Result<ServerIdentity> {
    let marker: Value =
        serde_json::from_str(marker).with_context(|| "remote install.json is not valid JSON")?;
    let source_server_id = marker
        .get("installationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("remote install.json is missing installationId")?;
    if let Err(error) = uuid::Uuid::parse_str(source_server_id) {
        bail!("remote install.json installationId is not a UUID: {error}");
    }
    Ok(ServerIdentity {
        source_server_id: source_server_id.to_owned(),
        os_user,
        hostname,
        workspace_path,
    })
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
#[path = "source_server_identity_tests.rs"]
mod tests;
