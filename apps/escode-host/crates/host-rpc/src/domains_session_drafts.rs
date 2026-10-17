//! `escode-session` 的 deferred 草稿关闭/提升（从 `domains_session_write` 拆出）。

use serde_json::Value;

use crate::domains_task_support::first_object;
use crate::session::{MethodHandler, handler_rpc};
use crate::wire::Wire;

use super::domains_session_write::{SessionWriteContext, forget_draft, forward};

/// `escode-session.closeDeferredDraftSession`。
pub(super) fn close_deferred_draft_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            let mut conditional = params.clone();
            conditional["expectedPersistence"] = Value::from("deferred");
            // 失败（含旧 Agent 拒绝 expectedPersistence）回 false：保留旧 session 并让 UI 新建草稿，
            // 不能退回无条件 close（可能关掉刚被其他客户端提升的 active task）。
            let closed = match forward(&context, "closeSession", &conditional).await {
                Ok(result) => result["closed"].as_bool().unwrap_or(true),
                Err(_) => false,
            };
            if closed {
                forget_draft(&context, &params);
            }
            Ok(Wire::Object(Value::Bool(closed)))
        })
    })
}

/// `escode-session.promoteDeferredDraftSession`。
pub(super) fn promote_deferred_draft_handler(context: SessionWriteContext) -> MethodHandler {
    handler_rpc(move |arg: Wire| {
        let params = first_object(arg);
        let context = context.clone();
        Box::pin(async move {
            // 清除草稿标记：此后该会话的状态变更不再被挡在索引之外（TS 另通知 syncer 建订阅，
            // 事件面落地前由下一次写入口/快照读写回索引）。
            forget_draft(&context, &params);
            Ok(Wire::Undefined)
        })
    })
}
