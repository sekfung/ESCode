//! 用户命令面的工作流 run 操作（docs/specs/rust-v4-command-gaps.md）：
//! - `cancelBackgroundWork {workId: dwfrun-…}`：run 卡 / 详情页的「取消」（TS runtime.cancelBackgroundTask，
//!   initiator = user）。修复：Rust 原先只认子代理与后台 Bash，对工作流 run 回 `noop`，取消按钮无效。
//! - `resumeWorkflowRun {workId, name?}`：详情页「恢复」（TS app.resumeWorkflowRun + 追踪重臂）。
//!
//! 两者都由工作流宿主执行。宿主处理中会反向请求会话 owner（`actor.create`、`workflowRuns.prior`），
//! 所以不能在 actor 里同步等应答：与 `mcp/list` 一样登记为辅助请求、后台调宿主，结果经
//! `Event::AuxiliaryDone` 回到 actor 再作为本次 `v4/command` 的应答发出。
use super::{Engine, auxiliary::Auxiliary};
use crate::{
    contract::{Event, EventSink},
    domain::protocol::{Command, Request},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const WORKFLOW_RUN_PREFIX: &str = "dwfrun-";
const RESUME_REJECTED_PREFIX: &str = "fault.command.workflowRunResumeRejected.";
const CANCEL_REJECTED_PREFIX: &str = "fault.command.backgroundWorkCancelRejected.";
/// TS 取消拒绝的 reason 去掉 `background_task_` 前缀进 reasonCode（BACKGROUND_WORK_CANCEL_REJECTED_FAULT_PREFIX）。
const BACKGROUND_TASK_REASON_PREFIX: &str = "background_task_";
const HOST_FAILURE: &str = "fault.command.executionFailed";

/// 需要宿主往返的 `v4/command`（在请求分派处先于普通命令路径拦截）。
pub(super) fn is_workflow_run_command(params: &Value) -> bool {
    match params["type"].as_str() {
        Some("resumeWorkflowRun") => true,
        Some("cancelBackgroundWork") => params["payload"]["workId"]
            .as_str()
            .is_some_and(|work| work.starts_with(WORKFLOW_RUN_PREFIX)),
        _ => false,
    }
}

impl Engine {
    pub(super) async fn start_workflow_run_command(&mut self, request: &Request) -> Result<()> {
        let c: Command =
            serde_json::from_value(request.params.clone()).context("Invalid command envelope")?;
        ensure!(
            !c.command_id.is_empty() && !c.client_id.is_empty() && c.issued_at.is_finite(),
            "Invalid command identity"
        );
        ensure!(self.auxiliary.len() < 16, "Too many auxiliary requests");
        let session = c.session_id.clone().context("Session id required")?;
        self.ensure_session(&session).await?;
        let revision = self.sessions[&session].revision;
        let work = c.payload["workId"]
            .as_str()
            .filter(|w| !w.trim().is_empty())
            .context("workId required")?
            .to_owned();
        let resume = c.kind == "resumeWorkflowRun";
        let (method, params) = if resume {
            let mut params = json!({ "session": session, "cwd": self.workspace_path, "workId": work });
            if let Some(name) = c.payload["name"].as_str() {
                params["name"] = name.into();
            }
            ("run.resume", params)
        } else {
            ("run.cancel", json!({ "session": session, "workId": work }))
        };
        let id = format!("workflow-run:{}", self.clock.id());
        self.auxiliary.insert(
            id.clone(),
            Auxiliary {
                request: request.id.clone(),
                cancel: CancellationToken::new(),
                operation: None,
                session: None,
            },
        );
        let sink = EventSink {
            session_id: id.clone(),
            run_id: id,
            tx: self.events.clone(),
        };
        let tools = self.tools.clone();
        tokio::spawn(async move {
            let ack = match tools.workflow_run(method, params).await {
                Ok(result) if resume => resume_ack(&c, revision, &result),
                Ok(result) => cancel_ack(&c, revision, &result),
                Err(error) => rejected(&c, revision, HOST_FAILURE.to_owned(), Some(&error.to_string())),
            };
            let _ = sink.send(Event::AuxiliaryDone { result: Ok(ack) }).await;
        });
        Ok(())
    }
}

fn rejected(c: &Command, revision: u64, reason: String, message: Option<&str>) -> Value {
    // TS 网关：携带 reasonCode 的领域错误以 failed 上行，error.message 收进 ack.message。
    let mut ack = c.ack("failed", revision, Some(&reason));
    if let Some(message) = message {
        ack["message"] = message.into();
    }
    ack
}

/// 宿主 `port.resume` 结果：`{ok:true, runId}` 或 `{ok:false, reason, message?}`。
fn resume_ack(c: &Command, revision: u64, result: &Value) -> Value {
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
fn cancel_ack(c: &Command, revision: u64, result: &Value) -> Value {
    let reason = if result["ok"] != true {
        Some(result["reason"].as_str().unwrap_or("background_task_not_found"))
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
