//! 远端 owner 的项目目录探测（S5-e）。
//!
//! 项目容器与 roots 仍由本机 Rust Host 的 tasks-index 持有；远端只负责文件系统事实
//! （`file.stat` / `file.readdir`）。没有在线 Source 时显式 `project.host-offline`，
//! 不落本机路径、不回退本地文件服务。

use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::source_server_client::{RemoteCallError, RemoteChannelClient};
use escode_host_services::source_server_connections::SourceServerConnections;

use crate::remote_host_id::source_id_from_host_id;
use crate::session::ChannelResult;
use crate::wire::Wire;

/// 已解析的远端项目 owner。
pub(crate) struct RemoteHost {
    pub host_id: String,
    pub client: Arc<RemoteChannelClient>,
}

fn rejected(code: &str) -> (String, String) {
    ("Rejected".to_owned(), code.to_owned())
}

fn call_error(error: anyhow::Error) -> (String, String) {
    match error.downcast_ref::<RemoteCallError>() {
        Some(remote) => (remote.name.clone(), remote.message.clone()),
        None => ("Error".to_owned(), format!("{error:#}")),
    }
}

/// `hostId` → 在线远端；`local` 返回 `None`。断线显式失败，不创建替代连接。
pub(crate) fn resolve_remote(
    connections: &SourceServerConnections,
    host_id: &str,
) -> Result<Option<RemoteHost>, (String, String)> {
    if host_id.is_empty() || host_id == "local" {
        return Ok(None);
    }
    let source_id =
        source_id_from_host_id(host_id).ok_or_else(|| rejected("project.host-offline"))?;
    let client = connections
        .online_client(source_id)
        .ok_or_else(|| rejected("project.host-offline"))?;
    Ok(Some(RemoteHost {
        host_id: host_id.to_owned(),
        client,
    }))
}

impl RemoteHost {
    async fn call_with_arg(
        &self,
        channel: &str,
        method: &str,
        arg: Option<WireValue>,
    ) -> Result<Value, (String, String)> {
        self.client
            .call(channel, method, arg)
            .await
            .map(|value| value.to_json().unwrap_or(Value::Null))
            .map_err(call_error)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, (String, String)> {
        let arg = WireValue::from(Wire::Array(vec![Wire::Object(params)]));
        self.call_with_arg("file", method, Some(arg)).await
    }

    async fn call_raw(
        &self,
        channel: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, (String, String)> {
        let arg = if params.is_null() {
            None
        } else {
            Some(WireValue::from(Wire::Array(vec![Wire::Object(params)])))
        };
        self.call_with_arg(channel, method, arg).await
    }

    /// 远端目录列表；错误保持远端 `name` / `message`，交给调用方原样上抛。
    pub(crate) async fn readdir(&self, path: &str, include_hidden: Option<bool>) -> ChannelResult {
        let mut params = json!({ "path": path });
        if let Some(include_hidden) = include_hidden {
            params["includeHidden"] = json!(include_hidden);
        }
        let entries = self.call("readdir", params).await?;
        Ok(Wire::Object(json!({ "path": path, "entries": entries })))
    }

    /// 远端目录存在性：目录 = available；ENOENT/ENOTDIR = missing；其余 = unavailable。
    pub(crate) async fn directory_status(&self, path: &str) -> &'static str {
        match self.call("stat", json!({ "path": path })).await {
            Ok(value) if value["type"] == json!("directory") => "available",
            Ok(_) => "missing",
            Err((_, message)) if message.contains("ENOENT") || message.contains("ENOTDIR") => {
                "missing"
            }
            Err(_) => "unavailable",
        }
    }

    /// 远端 owner 的稳定 workspace identity：`<hostId>:<posix-normalized-path>`。
    pub(crate) fn workspace_identity(&self, path: &str) -> String {
        workspace_identity_for_host(&self.host_id, path)
    }

    /// 远端 `os.homedir()`：缺省目录浏览的起点。
    pub(crate) async fn home(&self) -> Result<String, (String, String)> {
        let info = self.call_raw("system", "info", Value::Null).await?;
        Ok(info["homedir"].as_str().unwrap_or_default().to_owned())
    }
}

/// 只依赖 host id 的身份构造（读库路径不需要在线连接）。
pub(crate) fn workspace_identity_for_host(host_id: &str, path: &str) -> String {
    format!("{host_id}:{}", normalize_identity_path(path))
}

/// 与 `buildServerRemoteWorkspaceIdentity` 相同：分隔符转 `/`、折叠重复、去首尾 `/` 后补一个。
fn normalize_identity_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let mut collapsed = String::with_capacity(replaced.len());
    for character in replaced.chars() {
        if character == '/' && collapsed.ends_with('/') {
            continue;
        }
        collapsed.push(character);
    }
    format!("/{}", collapsed.trim_matches('/'))
}

/// 项目操作的目录事实来源：本地直接访问文件系统，远端经在线 Source 的 `file` channel。
pub(crate) enum DirectoryProbe<'a> {
    Local,
    Remote(&'a RemoteHost),
}

impl DirectoryProbe<'_> {
    pub(crate) fn host_id(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::Remote(host) => &host.host_id,
        }
    }

    pub(crate) async fn status(&self, path: &str) -> &'static str {
        match self {
            Self::Local => local_status(path).await,
            Self::Remote(host) => host.directory_status(path).await,
        }
    }

    pub(crate) async fn is_directory(&self, path: &str) -> bool {
        self.status(path).await == "available"
    }

    /// 远端 prepareSession 返回身份；本地不推断 attachment 身份。
    pub(crate) fn workspace_identity(&self, path: &str) -> Option<String> {
        match self {
            Self::Local => None,
            Self::Remote(host) => Some(host.workspace_identity(path)),
        }
    }
}

async fn local_status(path: &str) -> &'static str {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_dir() => "available",
        Ok(_) => "missing",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_normalization_matches_shared_builder() {
        assert_eq!(
            normalize_identity_path("/root/workspace"),
            "/root/workspace"
        );
        assert_eq!(
            normalize_identity_path("/root//workspace/"),
            "/root/workspace"
        );
        assert_eq!(normalize_identity_path("C:\\work\\repo"), "/C:/work/repo");
        assert_eq!(normalize_identity_path(""), "/");
    }
}
