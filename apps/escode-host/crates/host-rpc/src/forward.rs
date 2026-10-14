//! 服务域共用的运行时转发路径：服务级参数 → workspace 路由 → 实例 → 请求/应答 → 抽取。
//! 已迁移的 channel 都经这里转发（rust-u2-host-skeleton.md §5）。

use serde_json::{Value, json};

use crate::session::ChannelResult;
use crate::wire::Wire;

/// 从服务级参数里抽出运行时路由信息，并构造 CLI 形状的请求参数。
///
/// - 路由键（workspacePath / workspaceIdentity / remoteSessionId / dataDir / config / runtimePolicy）
///   只用于 Host 侧建实例，不转发；
/// - `workspace` 引用按 TS `buildWorkspaceRef` 构造（运行时用它做 workspace 校验）；
/// - 其余字段原样转发，`null` 一律省略（运行时的可选字段解析严格拒绝 null）。
pub struct RuntimeTarget {
    identity: String,
    descriptor: escode_host_control::WorkspaceDescriptor,
    request: Value,
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
    let identity = pick("workspaceIdentity")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_owned))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| path.clone());
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
    // 可选字段缺省即省略（运行时 workspace 引用解析严格拒绝 null）。
    let mut workspace = json!({
        "workspacePath": path,
        "workspaceIdentity": identity,
        "workspaceKey": identity,
    });
    if let Some(remote_session_id) = remote_session_id {
        workspace["remoteSessionId"] = Value::String(remote_session_id);
    }
    request.insert("workspace".to_owned(), workspace);
    Ok(RuntimeTarget {
        identity,
        descriptor,
        request: Value::Object(request),
    })
}

/// 应答抽取：`Result` = 整个 `result`；`Field` = `result.<field>`；
/// `FieldOr` = `result.<field>` ?? 默认值（TS 的 `result.closed ?? true` 同形）。
#[derive(Clone, Copy)]
pub enum Extract {
    Result,
    Field(&'static str),
    FieldOr(&'static str, bool),
}

/// 把一次服务调用转发给该 workspace 的 runtime 实例。
pub async fn runtime_call(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    method: &str,
    params: Value,
    extract: Extract,
) -> ChannelResult {
    let target = runtime_target(params)?;
    let instance = instances
        .get(&target.identity, || options(&target.descriptor))
        .await
        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))?;
    let answer = instance
        .call(method, target.request, std::time::Duration::from_secs(30))
        .await
        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))?;
    if let Some(error) = answer.get("error") {
        return Err((
            "RuntimeError".to_owned(),
            error["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| error.to_string()),
        ));
    }
    let result = answer.get("result").cloned().ok_or_else(|| {
        (
            "Fatal".to_owned(),
            format!("runtime `{method}` answer lacks `result`: {answer}"),
        )
    })?;
    match extract {
        Extract::Result => Ok(Wire::Object(result)),
        Extract::Field(field) => result.get(field).cloned().map(Wire::Object).ok_or_else(|| {
            (
                "Fatal".to_owned(),
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
