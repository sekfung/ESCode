//! 服务域共用的运行时转发路径：服务级参数 → workspace 路由 → 实例 → 请求/应答 → 抽取。
//! 已迁移的 channel 都经这里转发（rust-u2-host-skeleton.md §5）。
//!
//! 错误形状与 TS 一致（评审：原实现统一回 `RuntimeError` 且丢掉 JSON-RPC `code`）：
//! - runtime 的 JSON-RPC error → `{name: "ESCodeProtocolClientError", message, code, data?}`
//!   （TS `ESCodeProtocolClientError`，`ChannelServer` 透传 `code`/`data`）；
//! - existing-only 且无运行时 → `{name: "Error", message: "ESCode Agent runtime is not running.",
//!   code: "ESCODE_AGENT_RUNTIME_UNAVAILABLE"}`（TS `createRuntimeUnavailableError`）。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::session::{RpcError, RpcResult};
use crate::wire::Wire;

/// TS `ESCODE_AGENT_RUNTIME_UNAVAILABLE_CODE`。
pub const RUNTIME_UNAVAILABLE_CODE: &str = "ESCODE_AGENT_RUNTIME_UNAVAILABLE";

/// 从服务级参数里抽出运行时路由信息，并构造 CLI 形状的请求参数。
///
/// - 路由键（workspacePath / workspaceIdentity / remoteSessionId / dataDir / config / runtimePolicy）
///   只用于 Host 侧建实例，不转发；
/// - `workspace` 引用按 TS `buildWorkspaceRef` 构造（运行时用它做 workspace 校验）；
/// - 其余字段原样转发，`null` 一律省略（运行时的可选字段解析严格拒绝 null）。
pub struct RuntimeTarget {
    pub identity: String,
    pub descriptor: escode_host_control::WorkspaceDescriptor,
    pub request: Value,
}

pub fn runtime_target(params: Value) -> Result<RuntimeTarget, (String, String)> {
    let object = params.as_object().cloned().unwrap_or_default();
    let workspace_ref = object.get("workspace").cloned().unwrap_or(Value::Null);
    let pick = |key: &str| {
        object
            .get(key)
            .filter(|value| !value.is_null())
            .cloned()
            .or_else(|| {
                workspace_ref
                    .get(key)
                    .filter(|value| !value.is_null())
                    .cloned()
            })
    };
    let path = pick("workspacePath")
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    if path.is_empty() {
        return Err((
            "InvalidRequest".to_owned(),
            "escode-session calls require workspacePath".to_owned(),
        ));
    }
    let raw_identity =
        pick("workspaceIdentity").and_then(|value| value.as_str().map(str::to_owned));
    // 身份键：`workspaceIdentity?.trim() || workspacePath`（AGENTS.md Workspace Identity）。
    let identity = raw_identity
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| path.clone(), str::to_owned);
    let remote_session_id =
        pick("remoteSessionId").and_then(|value| value.as_str().map(str::to_owned));
    let descriptor = escode_host_control::WorkspaceDescriptor {
        path: path.clone(),
        identity: Some(identity.clone()),
        data_dir: pick("dataDir").and_then(|value| value.as_str().map(str::to_owned)),
        config: pick("config").and_then(|value| value.as_str().map(str::to_owned)),
    };
    let mut request = object;
    for key in [
        "workspacePath",
        "workspaceIdentity",
        "remoteSessionId",
        "workspaceKey",
        "dataDir",
        "config",
        "runtimePolicy",
        "workspace",
    ] {
        request.remove(key);
    }
    request.insert(
        "workspace".to_owned(),
        workspace_ref_json(
            &path,
            raw_identity.as_deref(),
            remote_session_id.as_deref(),
            &identity,
        ),
    );
    Ok(RuntimeTarget {
        identity,
        descriptor,
        request: Value::Object(request),
    })
}

/// TS `buildWorkspaceRef`：`{workspacePath, workspaceIdentity, remoteSessionId, workspaceKey}`，
/// 缺省字段由 JSON 序列化丢弃（`workspaceIdentity` 是**原值**，不回填路径；`workspaceKey` 是身份键）。
pub fn workspace_ref_json(
    path: &str,
    identity: Option<&str>,
    remote_session_id: Option<&str>,
    key: &str,
) -> Value {
    let mut workspace = json!({"workspacePath": path});
    if let Some(identity) = identity {
        workspace["workspaceIdentity"] = Value::from(identity);
    }
    if let Some(remote) = remote_session_id {
        workspace["remoteSessionId"] = Value::from(remote);
    }
    workspace["workspaceKey"] = Value::from(key);
    workspace
}

/// 应答抽取：`Result` = 整个 `result`；`Field` = `result.<field>`；
/// `FieldOr` = `result.<field>` ?? 默认值（TS 的 `result.closed ?? true` 同形）。
#[derive(Clone, Copy)]
pub enum Extract {
    Result,
    Field(&'static str),
    FieldOr(&'static str, bool),
}

/// 运行时策略（TS `ESCodeAgentRuntimePolicy`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePolicy {
    StartIfNeeded,
    ExistingOnly,
}

impl RuntimePolicy {
    /// 入参 `runtimePolicy`（缺省 start-if-needed）。
    pub fn of(params: &Value) -> Self {
        if params["runtimePolicy"] == "existing-only" {
            Self::ExistingOnly
        } else {
            Self::StartIfNeeded
        }
    }
}

/// 取（或按策略启动）一个 workspace 的实例。
pub async fn runtime_instance(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    target: &RuntimeTarget,
    policy: RuntimePolicy,
) -> Result<Arc<escode_host_runtime::Instance>, RpcError> {
    match policy {
        RuntimePolicy::ExistingOnly => instances.live(&target.identity).ok_or_else(|| {
            RpcError::new("Error", "ESCode Agent runtime is not running.")
                .with("code", Value::from(RUNTIME_UNAVAILABLE_CODE))
        }),
        RuntimePolicy::StartIfNeeded => instances
            .get(&target.identity, || options(&target.descriptor))
            .await
            .map_err(|error| RpcError::new("Fatal", format!("{error:#}"))),
    }
}

/// 一次运行时请求（`timeout` 30 s 与旧实现同），把 JSON-RPC error 转成 TS 同形错误，返回 `result`。
pub async fn request(
    instance: &escode_host_runtime::Instance,
    method: &str,
    params: Value,
    trace: Option<Value>,
) -> Result<Value, RpcError> {
    request_with_timeout(
        instance,
        method,
        params,
        trace,
        std::time::Duration::from_secs(30),
    )
    .await
}

/// 运行时请求的失败分类：`Timeout`（期限内无应答，实例可能是 stale）与其余失败。
/// 两者对外都是同一个 [`RpcError`]；分类只给需要“回收 stale 实例后重试”的调用方用。
pub enum CallError {
    Timeout(RpcError),
    Failed(RpcError),
}

impl CallError {
    pub fn into_rpc(self) -> RpcError {
        match self {
            Self::Timeout(error) | Self::Failed(error) => error,
        }
    }
}

/// 同 [`request`]，超时由调用方指定（插件安装/市场刷新等长操作 TS 用 5 分钟）。
pub async fn request_with_timeout(
    instance: &escode_host_runtime::Instance,
    method: &str,
    params: Value,
    trace: Option<Value>,
    timeout: std::time::Duration,
) -> Result<Value, RpcError> {
    request_classified(instance, method, params, trace, timeout)
        .await
        .map_err(CallError::into_rpc)
}

/// 同 [`request_with_timeout`]，但失败带分类（见 [`CallError`]）。
pub async fn request_classified(
    instance: &escode_host_runtime::Instance,
    method: &str,
    params: Value,
    trace: Option<Value>,
    timeout: std::time::Duration,
) -> Result<Value, CallError> {
    let answer = instance
        .call_with(method, params, trace, timeout)
        .await
        .map_err(|error| {
            let rpc = RpcError::new("Fatal", format!("{error:#}"));
            if error
                .downcast_ref::<escode_host_runtime::RequestTimeout>()
                .is_some()
            {
                CallError::Timeout(rpc)
            } else {
                CallError::Failed(rpc)
            }
        })?;
    if let Some(error) = answer.get("error") {
        let message = error["message"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string());
        let mut rpc = RpcError::new("ESCodeProtocolClientError", message);
        if let Some(code) = error.get("code").filter(|code| !code.is_null()) {
            rpc = rpc.with("code", code.clone());
        }
        if let Some(data) = error.get("data").filter(|data| !data.is_null()) {
            rpc = rpc.with("data", data.clone());
        }
        return Err(CallError::Failed(rpc));
    }
    answer.get("result").cloned().ok_or_else(|| {
        CallError::Failed(RpcError::new(
            "Fatal",
            format!("runtime `{method}` answer lacks `result`: {answer}"),
        ))
    })
}

/// 同上，但允许在转发前注入额外字段（如 `includeExecutionCapabilities`，
/// 由运行时能力协商决定 —— TS agent 层同序：先 `runtime/capabilities` 再 `workspace/readPresentation`）。
pub async fn runtime_call_with(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    method: &str,
    params: Value,
    extra: Option<(&str, Value)>,
    extract: Extract,
) -> RpcResult {
    let mut target = runtime_target(params)?;
    if let (Some((key, value)), Value::Object(request)) = (extra, &mut target.request) {
        request.insert(key.to_owned(), value);
    }
    forward_target(instances, options, method, target, None, extract).await
}

/// 把一次服务调用转发给该 workspace 的 runtime 实例。
pub async fn runtime_call(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    method: &str,
    params: Value,
    extract: Extract,
) -> RpcResult {
    let target = runtime_target(params)?;
    forward_target(instances, options, method, target, None, extract).await
}

/// 已整形的运行时调用：路由取自服务级参数，请求体由调用方按 TS 字面量整形后**逐字**发出。
pub struct Shaped {
    pub method: &'static str,
    pub body: Value,
    /// 是否在请求体上补 `workspace` 引用（TS `buildWorkspaceRef`；只有 list/create/resume/subscribe 带）。
    pub with_workspace: bool,
    /// 信封上的 `trace`（TS `client.request(..., {trace: {traceId}})`）。
    pub trace: Option<Value>,
}

/// 按 [`Shaped`] 转发。
pub async fn runtime_call_shaped(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    routing: Value,
    shaped: Shaped,
    extract: Extract,
) -> RpcResult {
    let mut target = runtime_target(routing)?;
    let workspace = target.request.get("workspace").cloned();
    let mut body = shaped.body;
    if shaped.with_workspace
        && let (Some(workspace), Value::Object(record)) = (workspace, &mut body)
    {
        record.insert("workspace".to_owned(), workspace);
    }
    target.request = body;
    forward_target(
        instances,
        options,
        shaped.method,
        target,
        shaped.trace,
        extract,
    )
    .await
}

async fn forward_target(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    method: &str,
    target: RuntimeTarget,
    trace: Option<Value>,
    extract: Extract,
) -> RpcResult {
    let instance =
        runtime_instance(instances, options, &target, RuntimePolicy::StartIfNeeded).await?;
    let result = request(&instance, method, target.request, trace).await?;
    match extract {
        Extract::Result => Ok(Wire::Object(result)),
        Extract::Field(field) => result.get(field).cloned().map(Wire::Object).ok_or_else(|| {
            RpcError::new(
                "Fatal",
                format!("runtime `{method}` answer lacks `result.{field}`: {result}"),
            )
        }),
        Extract::FieldOr(field, fallback) => Ok(Wire::Object(
            result
                .get(field)
                .filter(|value| !value.is_null())
                .cloned()
                .unwrap_or(Value::Bool(fallback)),
        )),
    }
}
