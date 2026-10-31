//! 远端 scope 转发（S3-b，docs/specs/source-server-remote-rust-host.md §10.11）。
//!
//! 一个 RPC 连接绑定 0 或 1 个远端 scope；绑定后该连接的 channel 集合 = Host 本地允许表 +
//! 远端 `Initialize` 广播过的 channel 转发入口。转发只做类型转换与错误映射，不解释业务语义：
//! 远端是这些 channel 的事实 owner。
//!
//! 安全边界（spec §10.11）：远端 scope 连接绝不注册本地共享 channel（file / setting / terminal /
//! git / credential …）——它们操作 Host 本机路径。允许表由组合根显式给出，未列入即 `NotImplemented`。

use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_services::source_server::IdentityChangePending;
use escode_host_services::source_server_client::{RemoteCallError, RemoteChannelClient};
use escode_host_services::source_server_connections::ConnectionCanceled;
use escode_host_services::source_server_provider_provisioning::ProviderProvisioningFailure;

use crate::channel::{Channel, EventHandler, MethodHandler, handler_rpc};
use crate::session::RpcError;
use crate::wire::Wire;
use remote_scope_listener::{event_handler, request_arg};

/// 一个远端 workspace 的身份三元组（spec §5.1：身份 key = `workspaceIdentity`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteScope {
    pub source_server_id: String,
    pub profile_id: String,
    pub workspace_identity: String,
}

impl RemoteScope {
    /// 从 capability 申请的 body 解析；缺字段显式报错（不按路径猜测）。
    pub fn parse(value: &Value) -> Result<Self, String> {
        let field = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("remote scope requires {name}"))
        };
        Ok(Self {
            source_server_id: field("sourceServerId")?,
            profile_id: field("profileId")?,
            workspace_identity: field("workspaceIdentity")?,
        })
    }
}

/// 首连目标：Main 只提交 transport 目标；Profile 匹配/创建与 `sourceServerId` 回填都由 Host 完成。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteProfileRequest {
    pub profile_id: Option<String>,
    pub target: Option<Value>,
    /// 调用方 key（Main 的 resolve requestId）：用于按 caller 作用域取消在途首连。
    pub request_id: Option<String>,
}

impl RemoteProfileRequest {
    pub fn parse(body: &[u8]) -> Result<Self, RemoteProfileResolveError> {
        let value: Value = serde_json::from_slice(body).map_err(|error| {
            RemoteProfileResolveError::new(format!(
                "source server profile resolution body is not JSON: {error}"
            ))
        })?;
        let profile_id = value
            .get("profileId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let target = value
            .get("target")
            .filter(|target| target.is_object())
            .cloned();
        if profile_id.is_none() && target.is_none() {
            return Err(RemoteProfileResolveError::new(
                "source server profile resolution requires profileId or target",
            ));
        }
        let request_id = value
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Ok(Self {
            profile_id,
            target,
            request_id,
        })
    }
}

/// `POST /api/source-server-cancel-profile` 的 caller key；缺字段显式报错。
pub fn parse_cancel_request(body: &[u8]) -> Result<String, RemoteProfileResolveError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        RemoteProfileResolveError::new(format!("source server cancel body is not JSON: {error}"))
    })?;
    value
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| RemoteProfileResolveError::new("source server cancel requires requestId"))
}

/// 已解析的首连 Profile：sourceServerId 来自握手回填后的持久 Profile。
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedRemoteProfile {
    pub profile_id: String,
    pub source_server_id: String,
    pub target: Value,
}

/// 首连解析错误；身份变化必须以结构化字段回给 Main（spec §11.12/§12.2）。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteProfileResolveError {
    pub message: String,
    pub code: Option<String>,
    pub profile_id: Option<String>,
    pub pending_source_server_id: Option<String>,
    pub retryable: Option<bool>,
}

impl RemoteProfileResolveError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            profile_id: None,
            pending_source_server_id: None,
            retryable: None,
        }
    }

    pub fn from_anyhow(error: anyhow::Error) -> Self {
        if let Some(pending) = error.downcast_ref::<IdentityChangePending>() {
            return Self {
                message: pending.to_string(),
                code: Some("source-server-identity-change-pending".to_owned()),
                profile_id: Some(pending.profile_id.clone()),
                pending_source_server_id: Some(pending.pending_source_server_id.clone()),
                retryable: Some(false),
            };
        }
        if error.downcast_ref::<ConnectionCanceled>().is_some() {
            return Self {
                message: ConnectionCanceled.to_string(),
                code: Some(CONNECTION_CANCELED_CODE.to_owned()),
                profile_id: None,
                pending_source_server_id: None,
                retryable: Some(true),
            };
        }
        if let Some(failure) = error.downcast_ref::<ProviderProvisioningFailure>() {
            return Self {
                message: failure.message.clone(),
                code: Some(failure.code.to_owned()),
                profile_id: None,
                pending_source_server_id: None,
                retryable: Some(failure.retryable),
            };
        }
        Self::new(format!("{error:#}"))
    }

    pub fn status_code(&self) -> u16 {
        if self.code.as_deref() == Some("source-server-identity-change-pending") {
            409
        } else {
            400
        }
    }

    pub fn to_json(&self) -> Value {
        let mut value = serde_json::Map::new();
        value.insert("error".to_owned(), Value::from(self.message.clone()));
        if let Some(code) = &self.code {
            value.insert("code".to_owned(), Value::from(code.clone()));
        }
        if let Some(profile_id) = &self.profile_id {
            value.insert("profileId".to_owned(), Value::from(profile_id.clone()));
        }
        if let Some(source_server_id) = &self.pending_source_server_id {
            value.insert(
                "pendingSourceServerId".to_owned(),
                Value::from(source_server_id.clone()),
            );
        }
        if let Some(retryable) = self.retryable {
            value.insert("retryable".to_owned(), Value::from(retryable));
        }
        Value::Object(value)
    }
}

/// 从 stable identity 解析远端 workspace 路径（TS `parseRemoteWorkspaceIdentity` 的子集）。
///
/// `remote:<kind>:<authority…>:<posixPath>`，kind ∈ ssh/wsl/docker/server；WSL 允许可选 user 段。
/// 非远程 identity 或路径不以 `/` 开头时返回 `None`（调用方不得按路径猜测）。
pub fn remote_workspace_path(identity: &str) -> Option<&str> {
    let rest = identity.strip_prefix("remote:")?;
    let (kind, mut cursor) = rest.split_once(':')?;
    let authority_segments = match kind {
        "ssh" => 3,
        "wsl" | "docker" | "server" => 1,
        _ => return None,
    };
    for _ in 0..authority_segments {
        let (segment, remainder) = cursor.split_once(':')?;
        if segment.is_empty() {
            return None;
        }
        cursor = remainder;
    }
    // WSL 可选 user 段：路径必须以 "/" 开头，因此可无歧义地区分 legacy 无 user 格式。
    if kind == "wsl" && !cursor.starts_with('/') {
        let (segment, remainder) = cursor.split_once(':')?;
        if segment.is_empty() {
            return None;
        }
        cursor = remainder;
    }
    cursor.starts_with('/').then_some(cursor)
}

/// 已授权的远端 scope：scope + 已 `Initialize` 的客户端句柄。
#[derive(Clone)]
pub struct RemoteGrant {
    pub scope: RemoteScope,
    pub client: Arc<RemoteChannelClient>,
}

/// 建立（或复用）scope 连接。失败必须是显式错误：签发方据此拒绝签发 capability。
pub type RemoteScopeEnsurer = Arc<
    dyn Fn(
            RemoteScope,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Arc<RemoteChannelClient>, String>> + Send>,
        > + Send
        + Sync,
>;

/// 首连解析器：按目标 ensure Profile 并建立连接，返回回填后的 Profile/Source 标识。
pub type RemoteProfileResolver = Arc<
    dyn Fn(
            RemoteProfileRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<ResolvedRemoteProfile, RemoteProfileResolveError>,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

/// 首连取消：按 caller key 中止在途连接；返回是否命中（未知/已完成 key = false）。
pub type RemoteProfileCanceller = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 远端 scope 的 Host 本地允许表：这些 channel 仍由 Host 本机实现（不转发）。
/// 调用方主动取消首连（公共错误码，UI 归类为 canceled）。
pub const CONNECTION_CANCELED_CODE: &str = "remote-connection-canceled";

pub const REMOTE_SCOPE_HOST_CHANNELS: &[&str] = &["window-controller"];

/// Host 私有能力：远端即使广播也不得转发给 Renderer（Provider Provisioning 只允许 Host 执行）。
pub const REMOTE_SCOPE_PRIVATE_CHANNELS: &[&str] = &["provider-provisioning-target"];

/// 把远端广播的 channel 变成转发入口。远端未广播的 channel 不创建（不猜能力）。
pub fn forwarded_channels(client: Arc<RemoteChannelClient>) -> Vec<Channel> {
    let Some(advertised) = client.channel_names() else {
        // 未 Initialize 的客户端没有任何远端能力可转发；调用方应在授权前完成 Initialize。
        return Vec::new();
    };
    let mut channels = Vec::new();
    for (name, members) in escode_host_proto::TS_SERVICE_CHANNELS {
        if !advertised.contains(*name) || REMOTE_SCOPE_PRIVATE_CHANNELS.contains(name) {
            continue;
        }
        channels.push(build_channel(name, members, &client));
    }
    let known: std::collections::BTreeSet<&str> = escode_host_proto::TS_SERVICE_CHANNELS
        .iter()
        .map(|(channel, _)| *channel)
        .collect();
    for unknown in advertised
        .iter()
        .filter(|channel| !known.contains(channel.as_str()))
    {
        escode_host_control::logging::warn(
            "source-server",
            format!(
                "remote advertised channel {unknown} is not in the Host inventory; not forwarded"
            ),
        );
    }
    channels
}

fn build_channel(
    name: &'static str,
    members: &'static [&'static str],
    client: &Arc<RemoteChannelClient>,
) -> Channel {
    let mut methods: Vec<(&'static str, MethodHandler)> = Vec::new();
    let mut events: Vec<(&'static str, EventHandler)> = Vec::new();
    for member in members {
        if is_event(member) {
            events.push((*member, event_handler(name, member, Arc::clone(client))));
        } else {
            methods.push((*member, method_handler(name, member, Arc::clone(client))));
        }
    }
    Channel::new(name, methods).with_events(events)
}

/// 事件成员约定：`on` + 大写开头（TS `Event<T>` 成员的命名约定）。
fn is_event(member: &str) -> bool {
    member
        .strip_prefix("on")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|first| first.is_ascii_uppercase())
}

fn method_handler(
    channel: &'static str,
    method: &'static str,
    client: Arc<RemoteChannelClient>,
) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let client = Arc::clone(&client);
        Box::pin(async move {
            let value = client
                .call(channel, method, request_arg(arg))
                .await
                .map_err(remote_error)?;
            Ok(Wire::from(value))
        })
    })
}

/// 远端错误 → RPC 错误：保留 `name`/`message`，`data.code` 提到顶层（TS 客户端按 `code` 识别）。
pub(crate) fn remote_error(error: anyhow::Error) -> RpcError {
    let Some(remote) = error.downcast_ref::<RemoteCallError>() else {
        return RpcError::new("Error", format!("{error:#}"));
    };
    let mut rpc = RpcError::new(remote.name.clone(), remote.message.clone());
    if let Some(data) = &remote.data
        && let Some(code) = data.get("code")
    {
        rpc = rpc.with("code", code.clone());
    }
    rpc
}

/// 诊断用：scope 的 JSON 形状（日志不写 identity 内容以外的用户数据）。
pub fn scope_log_value(scope: &RemoteScope) -> Value {
    json!({
        "profileId": scope.profile_id,
        "sourceServerId": scope.source_server_id,
    })
}

#[path = "remote_scope_listener.rs"]
mod remote_scope_listener;

#[cfg(test)]
#[path = "remote_scope_tests.rs"]
mod tests;
