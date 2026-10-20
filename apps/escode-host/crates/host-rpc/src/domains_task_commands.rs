//! `escode-task` 远程 workspace replayable 命令队列（S8）：`enqueueTaskCommand` /
//! `promoteTaskCommand` / `cancelTaskCommand`。
//!
//! 状态机在 `task_commands.rs`；本文件只做参数解析、入队/提升后的 drain 调度与 Wire 收口。
//! owner/stale 校验、返回形状逐条对齐 TS `escodeTaskServiceAdapter.ts`。

use crate::domains_task_input::task_target;
use crate::domains_task_state::{StateContext, method};
use crate::session::MethodHandler;
use crate::wire::Wire;

pub(super) fn command_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("enqueueTaskCommand", enqueue(context)),
        ("promoteTaskCommand", promote(context)),
        ("cancelTaskCommand", cancel(context)),
    ]
}

fn enqueue(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let value = context.commands.enqueue(&target, &params)?;
        context.commands.schedule(target, "enqueue");
        Ok(Wire::Object(value))
    })
}

fn promote(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let value = context.commands.promote(&target, &params)?;
        // TS running 分支提前返回、不调度；其余情况把提升后的命令交给 drain。
        if value["command"]["status"] != "running" {
            context.commands.schedule(target, "promote");
        }
        Ok(Wire::Object(value))
    })
}

fn cancel(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = task_target(&context, &params)?;
        let value = context.commands.cancel(&target, &params)?;
        // TS `emitRuntimeCommandSnapshotUpdated`：只有真正删除时刷新快照。
        if value["canceled"] == true && value.get("reason").is_none() {
            let trace_id = params["traceId"]
                .as_str()
                .unwrap_or(&target.task_id)
                .to_owned();
            context.task_events.snapshot_updated(&target, &trace_id);
        }
        Ok(Wire::Object(value))
    })
}
