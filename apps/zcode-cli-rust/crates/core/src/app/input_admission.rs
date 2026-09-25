use super::Engine;
use crate::domain::protocol::Command;
use anyhow::{Context, Result};
use serde_json::{Value, json};

impl Engine {
    /// 自定义 slash 命令在 admission 前异步展开（TS runPromptTurn 的 customCommandPromptResolver）；
    /// 内置 `/compact`、`/init` 由 admission 自行处理。见 docs/specs/rust-custom-commands.md。
    pub(super) async fn command_prompt(&self, id: &str, c: &Command) -> Result<Option<String>> {
        let Some(text) = c.payload["text"].as_str().filter(|_| c.kind == "sendText") else {
            return Ok(None);
        };
        let trimmed = text.trim();
        if trimmed == "/compact"
            || trimmed.starts_with("/compact ")
            || crate::domain::builtin_prompt_command::resolve_builtin_prompt_command(
                trimmed,
                std::path::Path::new(&self.workspace_path),
            )
            .is_some()
        {
            return Ok(None);
        }
        self.tools
            .resolve_command(Some(id), text, &tokio_util::sync::CancellationToken::new())
            .await
    }
    /// `command` 为已展开的自定义命令提示词：模型收到展开结果，userInput 行、标题与 `#sess_*` 解析
    /// 仍用原文（TS displayInput）。
    pub(super) fn admit_input(
        &mut self,
        id: &str,
        c: &Command,
        shared: Option<String>,
        command: Option<String>,
    ) -> Result<(String, String)> {
        // `/init` 与 TS 一样展开成普通用户提示词（builtin-prompt-command.ts），不改变其余 admission 语义。
        let mut model_text = command;
        if c.kind == "sendText"
            && let Some(text) = c.payload["text"].as_str()
        {
            let text = text.trim();
            if text == "/compact" || text.starts_with("/compact ") {
                let mut compact = c.clone();
                compact.payload = json!({"text":text.strip_prefix("/compact").unwrap().trim()});
                return self.admit_compact(id, &compact);
            }
            // 修复：此前 `/init` 把展开后的提示词写回 payload，userInput 行与标题显示整段提示词；
            // TS 以 displayInput 保留原文，只有模型消息使用展开结果。
            if let Some(prompt) =
                crate::domain::builtin_prompt_command::resolve_builtin_prompt_command(
                    text,
                    std::path::Path::new(&self.workspace_path),
                )
            {
                model_text = Some(prompt);
            }
        }
        let selected = self.select(&c.payload, Some(self.session_selection(id)?))?;
        let mut content = match &model_text {
            Some(prompt) => {
                let mut payload = c.payload.clone();
                payload["text"] = prompt.clone().into();
                self.input_content(id, &payload)?
            }
            None => self.input_content(id, &c.payload)?,
        };
        if c.payload["_userSteer"] == true
            && let Some(text) = content.as_str()
        {
            content = crate::domain::prompt::user_steer(text).into();
        }
        self.apply_selection(id, selected)?;
        let s = self.sessions.get_mut(id).context("Session unavailable")?;
        // 输入携带的协作模式即会话模式（TS resolveExecutionState）；缺省保持会话现值。
        if let Some(mode) = c.payload["mode"]
            .as_str()
            .filter(|mode| matches!(*mode, "yolo" | "build" | "edit"))
            && s.mode != mode
        {
            s.mode = mode.into();
            s.revision += 1;
        }
        if let Some(plan) = c.payload["planEnabled"].as_bool()
            && s.plan_enabled != plan
        {
            s.plan_enabled = plan;
            s.revision += 1;
        }
        let boundary = (
            s.rows.len(),
            s.messages.len(),
            crate::domain::history::State::capture(s),
        );
        let now = self.clock.now();
        let turn = self.clock.id();
        let input = self.clock.id();
        let text = c.payload["text"].as_str().context("Input text missing")?;
        if c.kind == "sendGoalCommand" {
            s.goal = Some(crate::domain::goal::Goal::new(
                self.clock.id(),
                text.trim().into(),
                now,
            ));
            content = s.goal.as_ref().unwrap().prompt("goalContinue", None).into();
        } else if let Some(goal) = s.goal.as_mut().filter(|g| g.active()) {
            goal.start(now);
        }
        super::shared_context::attach(
            s,
            &c.payload,
            shared,
            Some(&format!("queue_{}", c.command_id)),
            &input,
        )?;
        let retained_messages = s.messages.len();
        s.run_id = Some(self.clock.id());
        s.phase = "running".into();
        s.last_error = None;
        s.updated_at = now;
        s.revision += 1;
        // TS 对 automation 执行会话传 titleGenerationEnabled=false：不请求模型，标题停在 first_input。
        let automation = crate::domain::cron::turn_automation_id(
            c.payload["automationId"].as_str(),
            &c.command_id,
        )
        .is_some();
        let first_input = s.title.is_empty();
        if first_input {
            // TS `titleFromInput(displayInput)`（docs/specs/rust-session-title.md）；附件为空文本时
            // 沿用既有回退（首个附件名），其余规则与 Node 逐字一致。
            let title_text = if text.trim().is_empty() {
                c.payload["attachments"][0]["fileName"]
                    .as_str()
                    .unwrap_or("Attachment")
            } else {
                text
            };
            s.title = crate::domain::session_title::title_from_input(title_text);
            s.title_source = "first_input".into();
        }
        // `/goal` 另需目标摘要标题（生成或兜底），即使不是首条输入。
        let goal_target = (c.kind == "sendGoalCommand")
            .then(|| s.goal.as_ref().map(|g| g.target_id.clone()))
            .flatten();
        if first_input || goal_target.is_some() {
            s.title_seed = Some(crate::domain::session_title::TitleSeed {
                entity: input.clone(),
                text: text.to_owned(),
                session: first_input,
                goal_target,
                automation,
            });
        }
        let mut header = s.row("turnHeader", &turn, &turn, now);
        header["origin"] = if c.payload["_historyRerun"] == true {
            "editRerun"
        } else {
            "userInput"
        }
        .into();
        header["state"] = "running".into();
        header["startedAt"] = now.into();
        header["sourceCommandId"] = c.command_id.clone().into();
        s.rows.push(header);
        let mut row = s.row("userInput", &turn, &input, now);
        row["text"] = text.into();
        if c.kind == "sendGoalCommand" {
            row["text"] = c.payload["displayText"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("/goal {text}"))
                .into();
        }
        row["origin"] = "realUser".into();
        row["sourceCommandId"] = c.command_id.clone().into();
        row["clientId"] = c.client_id.clone().into();
        if let Some(refs) = c.payload.get("attachments") {
            row["attachments"] = refs.clone();
        }
        s.rows.push(row);
        if c.kind == "sendGoalCommand" {
            let mut marker = s.row("timelineMarker", &turn, &self.clock.id(), now);
            marker["marker"] = json!({"type":"goalSet","objective":text});
            s.rows.push(marker);
        }
        if !s.background.is_empty() {
            let statuses = s
                .background
                .values()
                .map(|t| json!({"task_id":t.id,"status":t.status,"outputFile":t.output_file}))
                .collect::<Vec<_>>();
            s.append_message(json!({"role":"user","content":format!("<task-notification>{}</task-notification>", serde_json::to_string(&statuses)?)}));
        }
        // TS injectReferencedSessionContextReminderIntoMessageHistory：输入含 #sess_* 引用时提示可用
        // ReadSessionContext（docs/specs/rust-read-session-context.md）。
        if let Some(body) = crate::domain::session_context::referenced_reminder_body(text) {
            s.append_message(json!({"role":"user","content":crate::domain::plan_mode::wrap(&body),"_zcode_source":"referenced_session_context"}));
        }
        // TS 在 referenced reminder 之后注入跨日 reminder（docs/specs/rust-date-change.md）。
        super::goal_commands::date_change(s, self.clock.local_date());
        // TS buildRuntimeModeReminderBody：plan 开启时按节奏在用户正文前插入模式 reminder。
        if let Some(reminder) = crate::domain::plan_mode::mode_reminder(&s.messages, s.plan_enabled)
        {
            s.append_message(reminder);
        }
        // `_zcode_input`：真实用户输入及其是否满足记忆提取的散文门槛（按模型实际收到的正文计词）。
        let prose = crate::domain::memory::is_prose(model_text.as_deref().unwrap_or(text));
        s.append_message(json!({"role":"user","content":content,"_zcode_input":prose}));
        let mut payload = c.payload.clone();
        payload.as_object_mut().unwrap().remove("context_refs");
        // TS prompt-turn：本轮 automation 身份（显式或 automation- 前缀 commandId）与禁用工具面随输入固化。
        let automation = crate::domain::cron::turn_automation_id(
            c.payload["automationId"].as_str(),
            &c.command_id,
        );
        let requested: Vec<String> = c.payload["toolDisallowlist"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let disallowed = crate::domain::cron::turn_disallowlist(&requested, automation.as_deref());
        if let Some(id) = &automation {
            payload["automationId"] = id.clone().into();
        }
        if !disallowed.is_empty() {
            payload["toolDisallowlist"] = disallowed.into();
        }
        s.history
            .inputs
            .push(crate::domain::history::InputBoundary {
                entity: input.clone(),
                turn: turn.clone(),
                row: boundary.0,
                user_row: boundary.0 + 1,
                message: retained_messages,
                state: boundary.2,
                kind: c.kind.clone(),
                payload,
            });
        Ok((turn, input))
    }
    pub(super) fn new_turn_rows(&self, id: &str) -> Vec<Value> {
        let rows = &self.sessions[id].rows;
        rows[self.sessions[id].current_rows_start()..]
            .iter()
            .map(|row| json!({"op":"row.appended","row":row}))
            .collect()
    }
}
