//! 工作流 actor 会话（docs/specs/rust-dynamic-workflow.md「M2 设计」）：工作流宿主的远程 AgentRuntime 把每一轮
//! 交给这里。actor 是不进会话列表的子会话：系统提示词走 TS workflowActor 路径（身份段由宿主算好），工具面 =
//! 父会话全集减禁用列表加远程工具（`submit_result` / `escalate`，执行时回调宿主），模型 = run 的
//! `subagent_model` > pin > 父会话当前选择。`actor.turn` 的应答在这一轮结束时给出（最终回复 + 用量）。
use super::{Engine, Event};
use crate::domain::session::Session;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::oneshot;

pub(super) type ActorReply = oneshot::Sender<std::result::Result<Value, String>>;

/// 在飞的一轮：应答句柄与本轮累计用量。
pub(super) struct ActorTurn {
    pub reply: ActorReply,
    pub tokens: u64,
    pub requests: u64,
}

/// TS workflow_child runtime 的结构性缺席：`WORKFLOW_CHILD_DISALLOWED_TOOLS`（tool-allowlist.ts），
/// `subagents: { enabled: false }`（无 Agent / SendMessage），以及 child 不装定时任务端口（无 Cron*）。
const WORKFLOW_CHILD_ABSENT: [&str; 11] = [
    "CreateWorkflow",
    "AmendWorkflow",
    "SaveWorkflow",
    "ResumeWorkflowRun",
    "ResolveWorkflowQuestion",
    "Agent",
    "SendMessage",
    "CronCreate",
    "CronDelete",
    "CronList",
    "CronUpdate",
];

/// actor 的工具面：减去禁用列表，追加宿主裁决的远程工具。
pub(super) fn actor_definitions(actor: &Value, definitions: &mut Vec<Value>) {
    let disallowed: Vec<&str> = actor["disallowed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(WORKFLOW_CHILD_ABSENT)
        .collect();
    definitions.retain(|d| !disallowed.contains(&d["function"]["name"].as_str().unwrap_or("")));
    for tool in actor["remoteTools"].as_array().into_iter().flatten() {
        definitions.push(json!({
            "type": "function",
            "function": {
                "name": tool["name"],
                "description": tool["description"],
                "parameters": tool["parameters"],
            },
        }));
    }
}

/// TS NodeContextSourceAdapter.detectEnvInfo 未给有效 shell 时的回退名。
fn env_shell_name() -> String {
    ["SHELL", "ComSpec"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .map(|path| {
            path.rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_else(|| "unknown".into())
}

impl Engine {
    /// 宿主对 actor 会话的请求。`actor.turn` 的应答留到这一轮结束；其余立即应答。
    pub(super) async fn actor_request(
        &mut self,
        method: &str,
        params: &Value,
        reply: ActorReply,
    ) -> Result<()> {
        let actor = params["actorSession"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let result = match method {
            "actor.create" => self.actor_create(&actor, params).await,
            // 宿主重启后首次见到会话：取已持久化的 `workflowRuns` 归约态作为起点。
            "workflowRuns.prior" => {
                let session = params["session"].as_str().unwrap_or_default();
                match self.ensure_session(session).await {
                    Ok(_) => Ok(self.sessions.get(session).and_then(|s| s.workflow_runs.clone()).unwrap_or(Value::Null)),
                    Err(_) => Ok(Value::Null),
                }
            }
            "actor.turn" => return self.actor_turn(&actor, params, reply).await,
            "actor.cancel" => {
                if let Some(active) = self.active.get(&actor) {
                    active.cancel.cancel();
                }
                Ok(json!({ "ok": true }))
            }
            "actor.close" => {
                if let Some(turn) = self.actor_turns.remove(&actor) {
                    let _ = turn.reply.send(Ok(json!({ "cancelled": true })));
                }
                Ok(json!({ "ok": true }))
            }
            // resume 重水化（TS resumeFromStore）：从会话库重载；会话行不在即 SessionNotFound（宿主退回全新路径）。
            "actor.resume" => {
                let found = self.ensure_session(&actor).await.is_ok();
                Ok(json!({ "found": found }))
            }
            "actor.title" => {
                if let Some(session) = self.sessions.get_mut(&actor) {
                    session.title = params["title"].as_str().unwrap_or_default().into();
                }
                self.persist(&actor, None)
                    .await
                    .map(|_| json!({ "ok": true }))
            }
            "actor.tools" => {
                if let Some(session) = self.sessions.get_mut(&actor)
                    && let Some(config) = session.workflow_actor.as_mut()
                {
                    config["remoteTools"] = params["remoteTools"].clone();
                }
                Ok(json!({ "ok": true }))
            }
            other => Err(anyhow::anyhow!("Unsupported actor request: {other}")),
        };
        let _ = reply.send(result.map_err(|error| format!("{error:#}")));
        Ok(())
    }

    async fn actor_create(&mut self, actor: &str, params: &Value) -> Result<Value> {
        // resume 时同一个 actor 会话再造一次 runtime：已在内存或已落库就沿用（转录由 actor.resume 重水化），
        // 只刷新宿主给的工具面配置。
        if self.sessions.contains_key(actor) || self.ensure_session(actor).await.is_ok() {
            if let Some(config) = self
                .sessions
                .get_mut(actor)
                .and_then(|s| s.workflow_actor.as_mut())
            {
                config["remoteTools"] = params["remoteTools"].clone();
                config["disallowed"] = params["disallowed"].clone();
            }
            return Ok(json!({ "ok": true }));
        }
        let parent = params["session"]
            .as_str()
            .context("Workflow actor needs its parent session")?
            .to_owned();
        self.ensure_session(&parent).await?;
        // 模型：run 的 subagent_model > pin（`providerId/modelId`）> 父会话当前选择。
        let base = self.session_selection(&parent)?;
        // 宿主已按 TS workflowActorModelPolicy 算好（run 的 subagent_model > pin > 父会话当前选择）。
        let requested = if params["selection"].is_object() {
            json!({ "modelSelection": params["selection"] })
        } else {
            json!({})
        };
        let selection = self.select(&requested, Some(base))?;
        let parent_session = &self.sessions[&parent];
        let mut session = Session::new(
            actor.to_owned(),
            self.workspace.clone(),
            selection.provider_id,
            selection.model_id,
            selection.reasoning_level,
            self.clock.id(),
            self.clock.now(),
        );
        session.parent_id = Some(parent.clone());
        session.task_type = "workflow_child".into();
        session.listed = false;
        session.title = params["persona"]["name"]
            .as_str()
            .unwrap_or("workflow actor")
            .into();
        session.title_source = "custom".into();
        session.workspace_path = Some(self.workspace_path.clone());
        // TS actor runtime 的技能端口只扫用户 / 项目技能根（createNodeSkillAdapter），不含插件与内置技能包。
        session.skills = parent_session.skills.clone().map(|mut catalog| {
            catalog
                .skills
                .retain(|skill| skill.plugin_name.is_none() && skill.scope != "system");
            catalog
        });
        session.prompt_snapshot = parent_session.prompt_snapshot.clone();
        // TS actor runtime 不套用会话 shell 选择，环境段的 Shell 是 `basename(SHELL ?? ComSpec)`。
        if let Some(snapshot) = session.prompt_snapshot.as_mut() {
            snapshot.shell = env_shell_name();
        }
        session.mode = parent_session.mode.clone();
        session.plan_enabled = false;
        session.trace_id = Some(self.clock.id());
        session.workflow_actor = Some(json!({
            "identityPrompt": params["persona"]["identityPrompt"],
            "remoteTools": params["remoteTools"],
            "disallowed": params["disallowed"],
        }));
        self.tools.inherit_session(&parent, actor).await?;
        self.sessions.insert(actor.to_owned(), session);
        self.persist(actor, None).await?;
        Ok(json!({ "ok": true }))
    }

    async fn actor_turn(&mut self, actor: &str, params: &Value, reply: ActorReply) -> Result<()> {
        if !self.sessions.contains_key(actor) {
            let _ = reply.send(Err(format!("Unknown workflow actor session: {actor}")));
            return Ok(());
        }
        let text = params["input"].as_str().unwrap_or_default();
        let command = super::subagents::child_command(actor, &self.clock.id(), text);
        let (turn, _) = self.admit_input(actor, &command, None, None)?;
        self.actor_turns.insert(
            actor.to_owned(),
            ActorTurn {
                reply,
                tokens: 0,
                requests: 0,
            },
        );
        self.persist(actor, None).await?;
        self.tools
            .workflow_actor_event(
                actor,
                json!({ "kind": "model", "type": "model_request_started", "requestId": self.clock.id() }),
            )
            .await;
        self.start_run(actor, turn)
    }

    /// actor 会话的运行事件：用量累计进本轮，模型 / 工具开始 / 结束转交宿主合成 driver 的观察事件。
    pub(super) async fn forward_actor_event(&mut self, actor: &str, event: &Event) {
        if !self.actor_turns.contains_key(actor) {
            return;
        }
        let forwarded = match event {
            Event::ModelDone { usage, .. } => {
                self.actor_model_done(actor, usage);
                json!({ "kind": "model", "type": "model_request_completed", "requestId": self.clock.id() })
            }
            Event::ToolStart { call, .. } => {
                let input = call["function"]["arguments"]
                    .as_str()
                    .and_then(|a| serde_json::from_str::<Value>(a).ok())
                    .unwrap_or(Value::Null);
                json!({
                    "kind": "toolStart",
                    "callId": call["id"],
                    "name": call["function"]["name"],
                    "input": input,
                })
            }
            Event::ToolDone { id, failed, .. } => {
                json!({ "kind": "toolEnd", "callId": id, "error": failed })
            }
            _ => return,
        };
        self.tools.workflow_actor_event(actor, forwarded).await;
    }

    /// 本轮的一次模型请求结束：累计用量（TS TurnResult.usage 的 totalTokens / modelRequestCount）。
    fn actor_model_done(&mut self, actor: &str, usage: &Value) {
        if let Some(turn) = self.actor_turns.get_mut(actor) {
            turn.requests += 1;
            turn.tokens += usage["prompt_tokens"].as_u64().unwrap_or(0)
                + usage["completion_tokens"].as_u64().unwrap_or(0);
        }
    }

    /// 本轮结束：把最终回复与用量交回宿主（取消 / 失败按 TS executeTurn 的 reject 语义回报）。
    pub(super) fn finish_actor_turn(&mut self, actor: &str) {
        let Some(turn) = self.actor_turns.remove(actor) else {
            return;
        };
        let Some(session) = self.sessions.get(actor) else {
            let _ = turn.reply.send(Ok(json!({ "cancelled": true })));
            return;
        };
        let usage =
            json!({ "totalTokens": turn.tokens, "modelRequestCount": turn.requests.max(1) });
        let result = match session.phase.as_str() {
            "completedSuccess" => {
                let response = session
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "assistant")
                    .and_then(|m| m["content"].as_str())
                    .unwrap_or("")
                    .to_owned();
                json!({ "response": response, "usage": usage })
            }
            "completedInterrupted" => json!({ "cancelled": true, "usage": usage }),
            _ => {
                let message = session
                    .last_error
                    .as_ref()
                    .and_then(|e| e["message"].as_str())
                    .unwrap_or("Subagent turn failed")
                    .to_owned();
                let mut error = json!({ "message": message });
                // 模型失败的分类（TS AiSdkModelAdapterError 的 code / context）：driver 据此决定停 run、重驱或 ContextLimit。
                if let Some(failure) = &session.last_model_failure {
                    if let Some(raw) = &failure.provider_message {
                        error["message"] = raw.clone().into();
                    }
                    error["code"] = failure.code.into();
                    error["context"] = json!({
                        "reason": failure.reason, "retryable": failure.retryable, "providerCode": failure.provider_code,
                        "retryAfterMs": failure.retry_after_ms, "providerId": session.provider, "modelId": session.model,
                    });
                }
                json!({ "error": error, "usage": usage })
            }
        };
        let _ = turn.reply.send(Ok(result));
    }
}
