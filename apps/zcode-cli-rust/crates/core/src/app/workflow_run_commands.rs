//! 用户命令面的工作流 run 操作（docs/specs/rust-v4-command-gaps.md）：
//! - `cancelBackgroundWork {workId: dwfrun-…}`：run 卡 / 详情页的「取消」（TS runtime.cancelBackgroundTask，
//!   initiator = user）。修复：Rust 原先只认子代理与后台 Bash，对工作流 run 回 `noop`，取消按钮无效。
//! - `resumeWorkflowRun {workId, name?}`：详情页「恢复」（TS app.resumeWorkflowRun + 追踪重臂）。
//! - `startSavedWorkflow {name, scope?, args?}`：设置页「已保存工作流」的「运行」（TS
//!   app.startSavedWorkflow，中枢直接启动）。
//!
//! 三者都由工作流宿主执行。宿主处理中会反向请求会话 owner（`actor.create`、`workflowRuns.prior`），
//! 所以不能在 actor 里同步等应答：与 `mcp/list` 一样登记为辅助请求、后台调宿主，结果经
//! `Event::AuxiliaryDone` 回到 actor 再作为本次 `v4/command` 的应答发出。
//!
//! 直接启动另有一条时序：宿主只做「零会话副作用」段（解析 / 校验 / 编译 / 工作副本 / `port.submit`），
//! 启动轮由会话 owner 落，追踪必须晚于启动轮重臂（否则很快结算的 run 的通知会跑到启动轮前面）——
//! 因此拆成 `run.startSaved` → `Event::WorkflowLaunchTurn` → `run.track` 三步。
use super::{Engine, auxiliary::Auxiliary};
use crate::{
    contract::{Event, EventSink},
    domain::protocol::{Command, Request},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const WORKFLOW_RUN_PREFIX: &str = "dwfrun-";
const RESUME_REJECTED_PREFIX: &str = "fault.command.workflowRunResumeRejected.";
const CANCEL_REJECTED_PREFIX: &str = "fault.command.backgroundWorkCancelRejected.";
const SAVED_START_REJECTED_PREFIX: &str = "fault.command.savedWorkflowStartRejected.";
/// TS 取消拒绝的 reason 去掉 `background_task_` 前缀进 reasonCode（BACKGROUND_WORK_CANCEL_REJECTED_FAULT_PREFIX）。
const BACKGROUND_TASK_REASON_PREFIX: &str = "background_task_";
pub(super) const HOST_FAILURE: &str = "fault.command.executionFailed";
/// 直接启动的两个宿主方法（`apps/zcode-cli/packages/cli/src/workflow-host-runs.ts`）。
const START_SAVED_METHOD: &str = "run.startSaved";
pub(super) const TRACK_SAVED_METHOD: &str = "run.track";
/// 宿主结果缺席 reason 时的兜底（TS `savedWorkflowStartRejectionReasonSchema` 的 start_failed）。
const START_SAVED_FALLBACK_REASON: &str = "start_failed";
/// 忙碌会话拒绝：与 TS `hasActiveOrQueuedTurnWork` 同一个判别键。
const SAVED_START_BUSY_REASON: &str = "session_busy";
/// 启动轮的 origin（TS `inputSource: "workflow_launch"` 同时定 turnHeaderOrigin 与 userInputOrigin）。
pub(super) const WORKFLOW_LAUNCH_ORIGIN: &str = "workflowLaunch";

/// 需要宿主往返的 `v4/command`（在请求分派处先于普通命令路径拦截）。
pub(super) fn is_workflow_run_command(params: &Value) -> bool {
    match params["type"].as_str() {
        Some("resumeWorkflowRun" | "startSavedWorkflow" | "amendWorkflowRunSettings") => true,
        Some("cancelBackgroundWork") => params["payload"]["workId"]
            .as_str()
            .is_some_and(|work| work.starts_with(WORKFLOW_RUN_PREFIX)),
        _ => false,
    }
}

impl Engine {
    /// 返回 `Some(batch)` 表示同步拒绝：本入口在请求分派处早返回，不会走到请求尾部的 outbox 冲刷，
    /// 所以这条应答必须由调用方就地发出（`session_busy` 的零副作用拒绝）。
    pub(super) async fn start_workflow_run_command(
        &mut self,
        request: &Request,
    ) -> Result<Option<Vec<Value>>> {
        let c: Command =
            serde_json::from_value(request.params.clone()).context("Invalid command envelope")?;
        ensure!(
            !c.command_id.is_empty() && !c.client_id.is_empty() && c.issued_at.is_finite(),
            "Invalid command identity"
        );
        ensure!(self.auxiliary.len() < 16, "Too many auxiliary requests");
        let session = c.session_id.clone().context("Session id required")?;
        self.ensure_session(&session).await?;
        if c.kind == "startSavedWorkflow" {
            return self.start_saved_workflow_command(request, c, session).await;
        }
        if c.kind == "amendWorkflowRunSettings" {
            return self.amend_workflow_settings_command(request, c, session).await;
        }
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
        Ok(None)
    }

    /// 中枢直接启动已保存的工作流（TS `startSavedWorkflowRun`）。
    ///
    /// 宿主只跑「零会话副作用」段（解析 / 校验 / 编译 / 工作副本 / `port.submit`）；启动轮由本 owner 落
    /// （[`Engine::apply_workflow_launch_turn`]），追踪必须晚于启动轮重臂——否则很快结算的 run 的
    /// `runSettled` 通知会跑到启动轮前面。三段因此串成
    /// `run.startSaved` → `Event::WorkflowLaunchTurn`（等 owner 的 ACK）→ `run.track`。
    async fn start_saved_workflow_command(
        &mut self,
        request: &Request,
        c: Command,
        session: String,
    ) -> Result<Option<Vec<Value>>> {
        let revision = self.sessions[&session].revision;
        // 忙碌预检（TS hasActiveOrQueuedTurnWork）：把启动排进在跑 turn 的队列要等 controlOnly 轮与
        // provider grammar 协调，属未来工作。拒绝在任何副作用之前——零 run、零消息、零事件、零任务。
        let busy = {
            let s = &self.sessions[&session];
            s.running() || !s.queue.is_empty() || s.queued_now.is_some()
        };
        if busy {
            return Ok(Some(vec![json!({
                "id": request.id,
                "result": saved_start_rejected(&c, revision, SAVED_START_BUSY_REASON, None),
            })]));
        }
        let name = c.payload["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .context("name required")?
            .to_owned();
        // 与 `run.resume` 同规矩：cwd 由 owner 给（宿主按它解析 <cwd>/.zcode/workflows/*.dwf.ts）。
        let mut params = json!({ "session": session, "cwd": self.workspace_path, "name": name });
        for key in ["scope", "args"] {
            if !c.payload[key].is_null() {
                params[key] = c.payload[key].clone();
            }
        }
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
            let launched = match tools.workflow_run(START_SAVED_METHOD, params).await {
                Ok(result) if result["ok"] == true => result,
                Ok(result) => {
                    let reason = result["reason"]
                        .as_str()
                        .unwrap_or(START_SAVED_FALLBACK_REASON);
                    let ack =
                        saved_start_rejected(&c, revision, reason, result["message"].as_str());
                    let _ = sink.send(Event::AuxiliaryDone { result: Ok(ack) }).await;
                    return;
                }
                Err(error) => {
                    let ack =
                        rejected(&c, revision, HOST_FAILURE.to_owned(), Some(&error.to_string()));
                    let _ = sink.send(Event::AuxiliaryDone { result: Ok(ack) }).await;
                    return;
                }
            };
            // 启动轮：唯一写会话的地方在 owner；本任务等它的 ACK 再重臂追踪。
            let (reply, reply_rx) = oneshot::channel();
            let launched_turn = Event::WorkflowLaunchTurn {
                session: session.clone(),
                command: Box::new(c.clone()),
                launched: launched.clone(),
                reply,
            };
            if sink.send(launched_turn).await.is_err()
                || reply_rx.await.map_err(|_| ()).is_err()
            {
                return;
            }
            // 提交之后的记账失败不回滚：run 已在飞、可在侧板取消，ACK 仍按成功回（TS 同规矩，只记日志）。
            let tool_call_id = launched["toolCallId"].as_str().unwrap_or_default();
            let _ = tools
                .workflow_run(
                    TRACK_SAVED_METHOD,
                    json!({ "session": session, "toolCallId": tool_call_id }),
                )
                .await;
            let _ = sink
                .send(Event::AuxiliaryDone {
                    result: Ok(saved_start_accepted(&c, revision, &launched)),
                })
                .await;
        });
        Ok(None)
    }
}

pub(super) fn rejected(c: &Command, revision: u64, reason: String, message: Option<&str>) -> Value {
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

impl Engine {
    /// 落中枢直接启动的启动轮（TS `emitControlOnlyUserTurn` + `persistWorkflowLaunchUserMessage`）：
    /// 标题 / userInput / controlOnly turnHeader / runtime history。会话状态的唯一写入点在 owner，
    /// 辅助任务只把宿主结果带过来。返回本次 `v4/command` 的 ACK（用改动后的 revision）。
    pub(super) async fn apply_workflow_launch_turn(
        &mut self,
        session: &str,
        c: &Command,
        launched: &Value,
    ) -> Result<Value> {
        let launch_input_id = launched["launchInputId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("launchInputId required")?
            .to_owned();
        let launch_text = launched["launchText"]
            .as_str()
            .context("launchText required")?
            .to_owned();
        let meta = launched["meta"].clone();
        let title_input = launched["titleInput"].as_str().unwrap_or_default().to_owned();
        let turn = self.clock.id();
        let now = self.clock.now();
        let s = self
            .sessions
            .get_mut(session)
            .context("Session unavailable")?;
        // TS ensureSessionPersisted：本会话是刚建的空会话，首个输入（工作流名）即标题。
        if s.title.is_empty() {
            s.title = crate::domain::session_title::title_from_input(&title_input);
            s.title_source = "first_input".into();
        }
        // TS leaveDraftAfterControlOnlyTurn：controlOnly 轮收口只在会话**仍是 draft** 时推进 phase
        // （成功 → completedSuccess），非 draft 会话上的控制轮不碰 session control。这一步同时让会话
        // 脱离 draft：可持久化（`Engine::persist` 对 draft 跳过提交）、可被 `session/list` 列出。
        if s.phase == "draft" {
            s.phase = "completedSuccess".into();
        }
        // 无 Agent 工时：turnHeader 直接落终态，且不带 activeMs / historyRoundCount（TS 对
        // controlOnly 传 undefined）。clientId 同样缺席——启动不是某个客户端的用户输入。
        let mut header = s.row("turnHeader", &turn, &turn, now);
        header["origin"] = WORKFLOW_LAUNCH_ORIGIN.into();
        header["executionKind"] = "controlOnly".into();
        header["state"] = "completedSuccess".into();
        header["startedAt"] = now.into();
        header["endedAt"] = now.into();
        header["sourceCommandId"] = launch_input_id.clone().into();
        header["workflowLaunch"] = meta.clone();
        s.rows.push(header.clone());
        let mut row = s.row("userInput", &turn, &launch_input_id, now);
        row["text"] = launch_text.clone().into();
        row["origin"] = WORKFLOW_LAUNCH_ORIGIN.into();
        row["sourceCommandId"] = launch_input_id.clone().into();
        // Rust 没有派生输入的 provenance，root 即本次启动（TS buildUserInputRow）。
        row["rootSourceCommandId"] = launch_input_id.clone().into();
        row["workflowLaunch"] = meta;
        s.rows.push(row.clone());
        // 模型下一回合读到的就是这条启动句。刻意**不带** `_zcode_input`：TS 这条消息是 synthetic
        // （extraction.ts 的散文门槛），也不计入 SessionStart 来源的 resume 判定；打上标记会同时在这
        // 两处与实际行为分叉（docs/specs/rust-v4-command-gaps.md）。
        s.append_message(json!({"role":"user","content":launch_text}));
        s.revision += 1;
        s.updated_at = now;
        self.publish(
            session,
            vec![
                json!({"op":"row.appended","row":header}),
                json!({"op":"row.appended","row":row}),
            ],
        )?;
        self.persist(session, None).await?;
        Ok(saved_start_accepted(c, self.sessions[session].revision, launched))
    }
}
