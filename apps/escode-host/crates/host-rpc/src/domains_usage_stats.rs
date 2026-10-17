//! `usage-stats.getAppUsageSnapshot`（docs/specs/rust-u2-misc-channels.md §1）。
//!
//! 与其余转发域的差别：**不按 workspace 路由**——usage 表在全局 session 库，TS 取任意一个已连接
//! client（`escodeAgentService.getAppUsageStats`），且**不拉起运行时**：无活跃 client 即抛
//! `no_active_workspace`（该错误在 UI 可见，`useUsageStats` 显示为错误态）。

use serde_json::{Map, Value};

use crate::channel::Channel;
use crate::forward;
use crate::session::{RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `new Error("no_active_workspace")` → `{name: "Error", message: "no_active_workspace"}`（无 `code`）。
fn no_active_workspace() -> RpcError {
    RpcError::new("Error", "no_active_workspace")
}

/// TS 字面量 `{range: params.range, timeZone: params.timeZone}`：只留这两个键，其余入参丢弃；
/// 缺省/null 一律省略（运行时解析严格，与 TS 的 JSON 序列化丢 `undefined` 同形）。
pub(crate) fn usage_request_body(params: &Value) -> Value {
    let mut body = Map::new();
    for key in ["range", "timeZone"] {
        if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
            body.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(body)
}

pub(crate) fn usage_stats_channel(instances: escode_host_runtime::Instances) -> Channel {
    let method = handler_rpc(move |arg: Wire| {
        let instances = instances.clone();
        Box::pin(async move {
            let params = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .unwrap_or(Value::Null);
            let instance = instances.live_any().ok_or_else(no_active_workspace)?;
            // 请求体不带 `workspace` 引用（TS 这里没有 buildWorkspaceRef）。
            let result = forward::request(
                &instance,
                "v4/usage/stats",
                usage_request_body(&params),
                None,
            )
            .await?;
            Ok(Wire::Object(result))
        })
    });
    Channel::new("usage-stats", vec![("getAppUsageSnapshot", method)])
}

#[cfg(test)]
#[path = "domains_usage_stats_tests.rs"]
mod tests;
