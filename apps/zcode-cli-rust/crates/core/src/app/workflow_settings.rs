//! GUI「配置」修订工作流 run 的两项设置（docs/specs/rust-v4-command-gaps.md「amendWorkflowRunSettings」）。
//!
//! 判定与执行全在工作流宿主用 TS 的同一段实现（core `applyWorkflowRunSettings`）完成：归属校验、可配置
//! 校验、三态归一、未改判定、脚本读取、编译、就地调并发或 `port.amend`、工作副本、提交 run。Rust 只做两件
//! 事：把模型目录（带会话当前选择的 `current`）递过去，以及落设置轮（或忙时暂存）。
//!
//! 与 `startSavedWorkflow` 同一条三段时序：`run.amendSettings` → `Event::WorkflowSettingsTurn`（等 owner
//! 落行 / 暂存）→ `run.track`。追踪必须晚于设置轮落定重臂——否则一个很快结算的 run 的完成通知会跑到设置轮
//! 前面。区别在于修订在忙会话上**不拒绝**：设置轮入 `Session::settings_turns` 延迟落，`deliver_workflow_notices`
//! 的 settings_turns 门保证通知排在它后面。
use super::{Engine, auxiliary::Auxiliary, workflow_run_commands::{
    HOST_FAILURE, TRACK_SAVED_METHOD, WORKFLOW_LAUNCH_ORIGIN, rejected,
}};
use crate::{
    contract::{Event, EventSink},
    domain::protocol::{Command, Request},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// 宿主方法（`apps/zcode-cli/packages/cli/src/workflow-host-runs.ts`）。
const AMEND_SETTINGS_METHOD: &str = "run.amendSettings";
/// 宿主结果缺席 reason 时的兜底（TS `workflowRunSettingsRejectionReasonSchema` 的 start_failed）。
const SETTINGS_FALLBACK_REASON: &str = "start_failed";
/// 拒绝 fault code 前缀（TS `WORKFLOW_RUN_SETTINGS_REJECTED_FAULT_PREFIX`）。
const SETTINGS_REJECTED_PREFIX: &str = "fault.command.workflowRunSettingsRejected.";

impl Engine {
    /// GUI「配置」修订：辅助请求跑宿主（不占 actor），结果回到 owner 落设置轮。
    pub(super) async fn amend_workflow_settings_command(
        &mut self,
        request: &Request,
        c: Command,
        session: String,
    ) -> Result<Option<Vec<Value>>> {
        let revision = self.sessions[&session].revision;
        // workId ≡ runId（与取消 / 恢复同一个身份等式）。
        let run_id = c.payload["workId"]
            .as_str()
            .filter(|w| !w.trim().is_empty())
            .context("workId required")?
            .to_owned();
        let mut params = json!({ "session": session, "cwd": self.workspace_path, "runId": run_id });
        // 两项设置三态原样透传：键在场即用户改过，`null` 是回到默认。
        for key in ["subagentModel", "maxConcurrency"] {
            if !c.payload[key].is_null() {
                params[key] = c.payload[key].clone();
            }
        }
        // 模型目录：Rust 的 Provider Registry 递过去，会话当前选择那条补 current（TS 端口的必需字段，
        // 决定 `model_unavailable` 诊断里的 `[current]` 标记与同名挂多 provider 的第 3 档判定）。
        params["models"] = self.session_model_catalog(&session);
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
            let applied = match tools.workflow_run(AMEND_SETTINGS_METHOD, params).await {
                Ok(result) if result["ok"] == true => result,
                Ok(result) => {
                    let reason = result["reason"]
                        .as_str()
                        .unwrap_or(SETTINGS_FALLBACK_REASON);
                    let ack = settings_rejected(&c, revision, reason, result["message"].as_str());
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
            // 设置轮：会话状态的唯一写入点在 owner（忙时暂存）。等 owner 落行 / 暂存后再重臂追踪——
            // 一个很快结算的 run 的 runSettled 通知，在被 deliver_workflow_notices 的 settings_turns 门
            // 挡住之前，不能先变成后台结果轮。
            let (reply, reply_rx) = oneshot::channel();
            let turn = Event::WorkflowSettingsTurn {
                session: session.clone(),
                command: Box::new(c.clone()),
                applied: applied.clone(),
                reply,
            };
            if sink.send(turn).await.is_err()
                || reply_rx.await.map_err(|_| ()).is_err()
            {
                return;
            }
            // 就地调并发那条路没有第二个后台任务（track=false），那个 run 本来就在追踪器里。
            if applied["track"] == true {
                let tool_call_id = applied["toolCallId"].as_str().unwrap_or_default();
                let _ = tools
                    .workflow_run(
                        TRACK_SAVED_METHOD,
                        json!({ "session": session, "toolCallId": tool_call_id }),
                    )
                    .await;
            }
            let _ = sink
                .send(Event::AuxiliaryDone {
                    result: Ok(settings_accepted(&c, revision, &applied)),
                })
                .await;
        });
        Ok(None)
    }

    /// 落 / 暂存 GUI「配置」的设置轮。修订已在宿主提交，这里只把这件事记进会话；busy 时入
    /// `Session::settings_turns` 等空闲。返回本次 `v4/command` 的 ACK（用改动后的 revision）。
    pub(super) async fn apply_workflow_settings_turn(
        &mut self,
        session: &str,
        c: &Command,
        applied: &Value,
    ) -> Result<Value> {
        // 忙（有活跃轮）时暂存：user 消息插不进正在跑的 turn（provider 语法），等空闲（Finished 后、
        // promote 前）flush。排队的 startNow 输入不构成忙——它还没起跑，设置轮此刻落仍先于它。
        if self.sessions[session].running() {
            self.sessions
                .get_mut(session)
                .context("Session unavailable")?
                .settings_turns
                .push(applied["turn"].clone());
            return Ok(settings_accepted(c, self.sessions[session].revision, applied));
        }
        self.write_settings_turn(session, &applied["turn"]).await?;
        Ok(settings_accepted(c, self.sessions[session].revision, applied))
    }

    /// 落设置轮（标题 / userInput / controlOnly turnHeader / runtime history）。与启动轮同形，
    /// 只是文本与 `workflowLaunch` 元数据来自宿主，且会话必已名下有 run（不做标题种子）。
    async fn write_settings_turn(&mut self, session: &str, turn_value: &Value) -> Result<()> {
        let text = turn_value["text"]
            .as_str()
            .filter(|t| !t.is_empty())
            .context("settings turn text required")?
            .to_owned();
        let meta = turn_value["meta"].clone();
        let turn = self.clock.id();
        let input_id = self.clock.id();
        let now = self.clock.now();
        let s = self
            .sessions
            .get_mut(session)
            .context("Session unavailable")?;
        if s.phase == "draft" {
            s.phase = "completedSuccess".into();
        }
        // 无 Agent 工时：turnHeader 直接落终态，与启动轮同一条 controlOnly 边。
        let mut header = s.row("turnHeader", &turn, &turn, now);
        header["origin"] = WORKFLOW_LAUNCH_ORIGIN.into();
        header["executionKind"] = "controlOnly".into();
        header["state"] = "completedSuccess".into();
        header["startedAt"] = now.into();
        header["endedAt"] = now.into();
        header["sourceCommandId"] = input_id.clone().into();
        header["workflowLaunch"] = meta.clone();
        s.rows.push(header.clone());
        let mut row = s.row("userInput", &turn, &input_id, now);
        row["text"] = text.clone().into();
        row["origin"] = WORKFLOW_LAUNCH_ORIGIN.into();
        row["sourceCommandId"] = input_id.clone().into();
        row["rootSourceCommandId"] = input_id.clone().into();
        row["workflowLaunch"] = meta;
        s.rows.push(row.clone());
        // 模型下一回合读到的就是这条设置句（与启动轮同一条 synthetic user 消息）。
        s.append_message(json!({"role":"user","content":text}));
        s.revision += 1;
        s.updated_at = now;
        self.publish(
            session,
            vec![
                json!({"op":"row.appended","row":header}),
                json!({"op":"row.appended","row":row}),
            ],
        )?;
        self.persist(session, None).await
    }

    /// 会话空闲时冲刷暂存的设置轮。必须在 `promote`（提升排队输入）**之前**调用：设置轮先于
    /// 排队输入与 run 通知落行（docs/specs/rust-v4-command-gaps.md「设置轮的时序」）。
    pub(super) async fn flush_settings_turns(&mut self, id: &str) -> Result<()> {
        let pending = {
            let Some(s) = self.sessions.get(id) else {
                return Ok(());
            };
            if s.running() || s.settings_turns.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut self.sessions.get_mut(id).unwrap().settings_turns)
        };
        for turn in pending {
            self.write_settings_turn(id, &turn).await?;
        }
        Ok(())
    }

    /// 会话模型目录：`registry.model_catalog()` 补 `current`（会话当前 provider/model 那一条为真）。
    fn session_model_catalog(&self, session: &str) -> Value {
        let Some(registry) = &self.registry else {
            return json!([]);
        };
        let (provider, model) = {
            let s = &self.sessions[session];
            (s.provider.clone(), s.model.clone())
        };
        let mut catalog = registry.model_catalog();
        for entry in &mut catalog {
            let current = entry["providerId"].as_str() == Some(provider.as_str())
                && entry["modelId"].as_str() == Some(model.as_str());
            entry["current"] = current.into();
        }
        json!(catalog)
    }
}

/// GUI「配置」修订的拒绝应答（TS V4WorkflowRunSettingsRejectedError）：reasonCode 带固定前缀，
/// `message` 直接进 ack.message（编译 / 模型解析 / 启动失败诊断都在里面），缺席时给可读兜底。
fn settings_rejected(c: &Command, revision: u64, reason: &str, message: Option<&str>) -> Value {
    let message = message
        .map(str::to_owned)
        .unwrap_or_else(|| format!("workflow run settings rejected: {reason}"));
    rejected(
        c,
        revision,
        format!("{SETTINGS_REJECTED_PREFIX}{reason}"),
        Some(&message),
    )
}

/// GUI「配置」修订的成功应答：`result` 给新 run 的两把关联键（runId ≡ backgroundTaskId ≡ 取消的
/// workId；toolCallId 联工具卡 → 详情页）；`supersededRunId` 只在旧 run 仍在飞、被这次调整停下时在场。
pub(super) fn settings_accepted(c: &Command, revision: u64, applied: &Value) -> Value {
    let mut ack = c.ack("accepted", revision, None);
    let mut result = json!({
        "type": "amendWorkflowRunSettings",
        "runId": applied["runId"],
        "toolCallId": applied["toolCallId"],
    });
    if let Some(superseded) = applied["supersededRunId"].as_str() {
        result["supersededRunId"] = superseded.into();
    }
    ack["result"] = result;
    ack
}
