//! `kind: "server"` 直连的两个 HTTP 响应解析（S5-f-2，§12.6.4）。
//!
//! 与 TS `serverRemoteInfoSchema` / `serverRemoteHostCapabilitySchema`
//! （`packages/shared/src/server-remote.ts`）同语义：必填字段缺失、协议版本不是 1、
//! capability 结构不成立都显式失败；不降级、不猜字段。

use anyhow::{Result, bail};
use serde_json::Value;

/// 与 TS `SERVER_REMOTE_PROTOCOL_VERSION` 一致。
pub const SERVER_REMOTE_PROTOCOL_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRemoteWorkspace {
    pub path: String,
    pub label: Option<String>,
    pub workspace_identity: Option<String>,
}

/// `/api/server-info` 中 Rust Host 需要的字段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRemoteInfo {
    pub server_id: String,
    pub name: Option<String>,
    pub version: String,
    pub protocol_version: u64,
    pub auth_required: bool,
    pub workspaces: Vec<ServerRemoteWorkspace>,
}

pub fn parse_server_info(value: &Value) -> Result<ServerRemoteInfo> {
    let server_id = required_text(value, "serverId")?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("invalid server-info: missing version"))?
        .to_owned();
    let protocol_version = value
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("invalid server-info: missing protocolVersion"))?;
    if protocol_version != SERVER_REMOTE_PROTOCOL_VERSION {
        bail!(
            "source-server-incompatible: protocol version {protocol_version} (host speaks {SERVER_REMOTE_PROTOCOL_VERSION})"
        );
    }
    let auth_required = value
        .get("authRequired")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("invalid server-info: missing authRequired"))?;
    let workspaces = value
        .get("workspaces")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("invalid server-info: missing workspaces"))?
        .iter()
        .map(parse_workspace)
        .collect::<Result<Vec<_>>>()?;
    let capabilities = value
        .get("capabilities")
        .ok_or_else(|| anyhow::anyhow!("invalid server-info: missing capabilities"))?;
    for key in ["desktopContinuous", "websocketRpc"] {
        if capabilities.get(key).and_then(Value::as_bool) != Some(true) {
            bail!("source-server-incompatible: capabilities.{key} is not supported");
        }
    }
    Ok(ServerRemoteInfo {
        server_id,
        name: optional_text(value, "name"),
        version,
        protocol_version,
        auth_required,
        workspaces,
    })
}

/// `/api/rpc-host-capability` 的 `capability`。
///
/// `expiresAt` 只做形状校验：capability 是一次性的，服务端在消费时判定过期，
/// Host 不据此缓存复用（重连必须重新申请）。
pub fn parse_host_capability(value: &Value) -> Result<String> {
    let expires_valid = value
        .get("expiresAt")
        .and_then(Value::as_i64)
        .is_some_and(|value| value > 0);
    if !expires_valid {
        bail!("invalid host capability: missing expiresAt");
    }
    required_text(value, "capability")
}

fn parse_workspace(value: &Value) -> Result<ServerRemoteWorkspace> {
    Ok(ServerRemoteWorkspace {
        path: required_text(value, "path")?,
        label: optional_text(value, "label"),
        workspace_identity: optional_text(value, "workspaceIdentity"),
    })
}

fn required_text(value: &Value, key: &str) -> Result<String> {
    optional_text(value, key).ok_or_else(|| anyhow::anyhow!("invalid response: missing {key}"))
}

fn optional_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "source_server_info_tests.rs"]
mod tests;
