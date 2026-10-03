//! V4 `createSelectionSideSession`：框选副屏（TS `handlers/selection-side-session.ts` +
//! core `createSelectionSideConversation`，docs/specs/rust-v4-command-gaps.md）。
//!
//! 修复：Rust 原先没有该命令，App 框选「问一问」副屏一律 `guard.capabilityUnsupported`。
//! 语义：从父会话创建隐藏的 `selection_side_chat` 子会话——继承父对话作为仅模型可见的参考
//! （运行中只截到当前轮的用户输入为止），追加一条副屏边界提醒；不复制行、Goal、队列与后台任务；
//! 子会话不进任务列表。可选 firstInput 直接在子会话起跑，父会话队列与运行态不受影响。
use super::Engine;
use crate::domain::{protocol::Command, session::Session};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub(super) const SIDE_CHAT_TASK_TYPE: &str = "selection_side_chat";
const SIDE_CHAT_TITLE: &str = "Selection side chat";
const SIDE_CHAT_SOURCE: &str = "selection_side_chat";
/// TS `SELECTION_SIDE_CHAT_BOUNDARY`（三句以空格连接）。
const SIDE_CHAT_BOUNDARY: &str = "The preceding conversation was inherited from the parent task for reference only. Do not continue the parent's active work automatically; answer only new questions sent in this side chat. Modify the workspace only when the user explicitly asks you to do so in this side chat.";
/// TS `SELECTION_SIDE_CHAT_RESTRICTED_COMMANDS`：副屏内不允许的命令。
pub(super) const SIDE_CHAT_RESTRICTED: [&str; 7] = [
    "sendGoalCommand",
    "pauseGoal",
    "resumeGoal",
    "editUserQuery",
    "retryTurn",
    "forkAssistant",
    "discardSharedContext",
];

impl Engine {
    pub(super) async fn create_selection_side_session(&mut self, c: &Command) -> Result<Value> {
        let parent_id = c.session_id.as_deref().context("Session id required")?;
        let parent = self.sessions.get(parent_id).context("Session unavailable")?;
        if parent.task_type == "subagent_child" {
            return Ok(c.ack("rejected", parent.revision, Some("guard.subagentReadOnly")));
        }
        let first = c.payload.get("firstInput").filter(|v| !v.is_null()).cloned();
        if let Some(input) = &first {
            ensure!(
                input.as_object().is_some_and(|o| o
                    .keys()
                    .all(|k| matches!(k.as_str(), "text" | "modelSelection"))),
                "Invalid side chat first input"
            );
            ensure!(
                input["text"].as_str().is_some_and(|t| !t.trim().is_empty()),
                "Side chat first input text required"
            );
        }
        let inherited = self.session_selection(parent_id)?;
        // 提交推荐只覆盖新 child 的完整选择，缺省保留父会话选择（TS 同注释）。
        let selection = match first.as_ref().and_then(|i| i.get("modelSelection")) {
            Some(model) => self.select(&json!({ "modelSelection": model }), Some(inherited))?,
            None => inherited,
        };
        let now = self.clock.now();
        let parent = &self.sessions[parent_id];
        let mut child = Session::new(
            format!("sess_{}", self.clock.id()),
            self.workspace.clone(),
            selection.provider_id.clone(),
            selection.model_id.clone(),
            selection.reasoning_level.clone(),
            self.clock.id(),
            now,
        );
        child.messages = side_chat_history(parent);
        child.context = if parent.context.offset <= child.messages.len() {
            parent.context.clone()
        } else {
            Default::default()
        };
        child.append_message(json!({
            "role": "user",
            "content": crate::domain::plan_mode::wrap(SIDE_CHAT_BOUNDARY),
            "_zcode_source": SIDE_CHAT_SOURCE,
        }));
        // 继承消息里的附件引用、冻结的 Skill 目录与提示快照，保证模型请求与父对话同一上下文。
        child.attachments = parent.attachments.clone();
        child.skills = parent.skills.clone();
        child.prompt_snapshot = parent.prompt_snapshot.clone();
        child.last_local_date = parent.last_local_date.clone();
        child.mode = parent.mode.clone();
        child.plan_enabled = parent.plan_enabled;
        child.followup_mode = parent.followup_mode.clone();
        child.workspace_path = parent.workspace_path.clone();
        child.workspace_directory = parent.workspace_directory.clone();
        child.trace_id = parent.trace_id.clone();
        child.parent_id = Some(parent_id.to_owned());
        child.task_type = SIDE_CHAT_TASK_TYPE.into();
        child.title = SIDE_CHAT_TITLE.into();
        child.title_source = "generated".into();
        child.listed = false;
        child.phase = "completedSuccess".into();
        let parent_revision = parent.revision;
        let id = child.id.clone();
        self.sessions.insert(id.clone(), child);
        match self.commit_side_session(c, parent_id, &id, selection, first, parent_revision).await {
            Ok(ack) => Ok(ack),
            Err(error) => {
                // 提交失败不能留下未持久化的 child runtime。
                self.sessions.remove(&id);
                Err(error)
            }
        }
    }

    async fn commit_side_session(
        &mut self,
        c: &Command,
        parent_id: &str,
        id: &str,
        selection: crate::contract::ModelIdentity,
        first: Option<Value>,
        parent_revision: u64,
    ) -> Result<Value> {
        self.apply_selection(id, selection)?;
        let mut ack = c.ack("accepted", parent_revision, None);
        ack["result"] = json!({ "type": "createSelectionSideSession", "sessionId": id });
        let mut turn = None;
        if let Some(input) = first {
            // 输入只落到 child：同一条信封改指 child，父会话 queue/CommandInbox 不参与 admission。
            let mut input_command = c.clone();
            input_command.session_id = Some(id.to_owned());
            input_command.kind = "sendText".into();
            input_command.payload = json!({ "text": input["text"], "requestedDelivery": "startNow" });
            if let Some(model) = input.get("modelSelection") {
                input_command.payload["modelSelection"] = model.clone();
            }
            let prompt = self.command_prompt(id, &input_command).await?;
            let (t, input_id) = self.admit_input(id, &input_command, None, prompt)?;
            turn = Some(t);
            ack["result"]["input"] = json!({ "delivery": "startNow", "inputId": input_id });
        }
        // child 与父命令的幂等 ACK 同一事务提交（同 forkAssistant）。
        self.persist(id, Some((c.key(), ack.clone()))).await?;
        self.acks.insert(c.key(), ack.clone());
        self.tools.inherit_session(parent_id, id).await?;
        self.publish(id, vec![])?;
        if let Some(turn) = turn {
            self.start_run(id, turn)?;
        }
        Ok(ack)
    }
}

/// TS `selectionSideChatHistoryMessages`：父会话运行中只继承到当前轮的真实用户输入（含）为止，
/// 不带入进行中的助手 / 工具消息；空闲时继承全部。
fn side_chat_history(parent: &Session) -> Vec<Value> {
    let mut messages = parent.messages.clone();
    if parent.running()
        && let Some(boundary) = parent.history.inputs.last()
        && boundary.message <= messages.len()
    {
        let end = messages[boundary.message..]
            .iter()
            .position(|m| m.get("_zcode_input").is_some())
            .map_or(boundary.message, |pos| boundary.message + pos + 1);
        messages.truncate(end);
    }
    messages
}
