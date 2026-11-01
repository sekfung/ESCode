//! `window-controller.onDynamicTaskUnreadSignal`（TS `windowHostControllerService.ts:340-380`）。
//!
//! 只把 `background_terminal` 的完成/失败转成一次性未读提示；列表快照与重同步都不重放
//! （TS 注释：`仅实时发送；列表快照与重同步不能重放一次完成提醒`）。

use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::session::EventStream;
use crate::wire::Wire;

use super::is_remote_workspace_identity;

/// TS `WindowHostTaskUnreadSignal`：`undefined` 字段整键省略。
pub(super) fn signal_of(event: &Value) -> Option<Value> {
    if event["type"] != "workspace_task_list_changed"
        || event["reason"] != "task_status_changed"
        || event["unreadSignal"] != "background_terminal"
    {
        return None;
    }
    let workspace_path = event["workspacePath"].as_str()?;
    let identity = event["workspaceIdentity"].as_str();
    let meta = event.get("taskMeta").filter(|meta| !meta.is_null())?;
    let status = meta["status"].as_str()?;
    if !matches!(status, "completed" | "error") {
        return None;
    }
    let task_id = event["taskId"]
        .as_str()
        .or_else(|| meta["taskId"].as_str())?;
    let updated_at = meta["updatedAt"].as_i64()?;
    let mut address = json!({"taskId": task_id, "workspacePath": workspace_path});
    if let Some(identity) = identity {
        address["workspaceIdentity"] = Value::from(identity);
    }
    Some(json!({
        "address": address,
        "updatedAt": updated_at,
        "status": status,
    }))
}

/// 一条连接一条流：事件按到达顺序投递，不做合并（未读提示是事实，不能被后来的事件吞掉）。
pub(super) fn listen(stream: EventStream) -> EventStream {
    Box::pin(stream.filter_map(|wire| async move {
        match wire {
            Wire::Object(value) => local_signal_of(&value).map(Wire::Object),
            _ => None,
        }
    }))
}

/// 本地 controller：远端 workspace 由连接级 controller 服务（§10.13），这里不重复认领。
pub(super) fn local_signal_of(event: &Value) -> Option<Value> {
    if event["workspaceIdentity"]
        .as_str()
        .is_some_and(is_remote_workspace_identity)
    {
        return None;
    }
    signal_of(event)
}

#[cfg(test)]
#[path = "unread_tests.rs"]
mod tests;
