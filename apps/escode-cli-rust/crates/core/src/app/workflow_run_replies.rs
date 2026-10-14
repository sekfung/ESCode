//! workflow run 命令的应答值构造（从 workflow_run_commands.rs 拆出）。

use crate::domain::protocol::Command;
use serde_json::{Value, json};

const RESUME_REJECTED_PREFIX: &str = "fault.command.workflowRunResumeRejected.";
const CANCEL_REJECTED_PREFIX: &str = "fault.command.backgroundWorkCancelRejected.";
const SAVED_START_REJECTED_PREFIX: &str = "fault.command.savedWorkflowStartRejected.";
/// TS 取消拒绝的 reason 去掉 `background_task_` 前缀进 reasonCode（BACKGROUND_WORK_CANCEL_REJECTED_FAULT_PREFIX）。
const BACKGROUND_TASK_REASON_PREFIX: &str = "background_task_";

pub(super) fn rejected(c: &Command, revision: u64, reason: String, message: Option<&str>) -> Value {
    // TS 网关：携带 reasonCode 的领域错误以 failed 上行，error.message 收进 ack.message。
    let mut ack = c.ack("failed", revision, Some(&reason));
    if let Some(message) = message {
        ack["message"] = message.into();
    }
    ack
}

/// 宿主 `port.resume` 结果：`{ok:true, runId}` 或 `{ok:false, reason, message?}`。
pub(super) fn resume_ack(c: &Command, revision: u64, result: &Value) -> Value {
    if result["ok"] == true {
        return c.ack("accepted", revision, None);
    }
    let reason = result["reason"].as_str().unwrap_or("unknown");
    // TS V4WorkflowRunResumeRejectedError：宿主没给诊断时用固定兜底句。
    let message = result["message"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("workflow run resume rejected: {reason}"));
    rejected(
        c,
        revision,
        format!("{RESUME_REJECTED_PREFIX}{reason}"),
        Some(&message),
    )
}

/// 宿主 `stopBackgroundTask` 结果；TS cancelBackgroundTask 把已终态也报成 `background_task_not_running`。
pub(super) fn cancel_ack(c: &Command, revision: u64, result: &Value) -> Value {
    let reason = if result["ok"] != true {
        Some(
            result["reason"]
                .as_str()
                .unwrap_or("background_task_not_found"),
        )
    } else if result["alreadyTerminal"] == true {
        Some("background_task_not_running")
    } else {
        None
    };
    match reason {
        None => c.ack("accepted", revision, None),
        Some(reason) => {
            let short = reason
                .strip_prefix(BACKGROUND_TASK_REASON_PREFIX)
                .unwrap_or(reason);
            // TS V4BackgroundWorkCancelRejectedError 的 message 原文。
            let message = format!(
                "background work {} was not cancelled: {reason}",
                c.payload["workId"].as_str().unwrap_or_default()
            );
            rejected(
                c,
                revision,
                format!("{CANCEL_REJECTED_PREFIX}{short}"),
                Some(&message),
            )
        }
    }
}

/// 直接启动的拒绝应答（TS V4SavedWorkflowStartRejectedError）：reasonCode 带固定前缀，`message`
/// 直接进 ack.message（编译诊断 / 实参诊断 / 文件问题都在里面），缺席时给可读兜底。
pub(super) fn saved_start_rejected(
    c: &Command,
    revision: u64,
    reason: &str,
    message: Option<&str>,
) -> Value {
    let message = message
        .map(str::to_owned)
        .unwrap_or_else(|| format!("saved workflow start rejected: {reason}"));
    rejected(
        c,
        revision,
        format!("{SAVED_START_REJECTED_PREFIX}{reason}"),
        Some(&message),
    )
}

/// 直接启动的成功应答：`result` 给 run 的两把关联键（runId ≡ backgroundTaskId ≡ 取消的 workId；
/// toolCallId 联工具卡 → 详情页）。
pub(super) fn saved_start_accepted(c: &Command, revision: u64, launched: &Value) -> Value {
    let mut ack = c.ack("accepted", revision, None);
    ack["result"] = json!({
        "type": "startSavedWorkflow",
        "runId": launched["runId"],
        "toolCallId": launched["toolCallId"],
    });
    ack
}
