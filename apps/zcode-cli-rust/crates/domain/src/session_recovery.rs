use super::session::Session;
use serde_json::{Value, json};
/// 工具结果（`finish_tool_row` 的输入）。
pub struct ToolResult {
    pub result: String,
    pub display: Option<Value>,
    pub failed: bool,
    pub denied: bool,
}
impl Session {
    /// 工具结果收口到行。被拒收口为 cancelled 且不写结果字段（TS settlePermission）；node_repl 图片只走行级 display，
    /// 行级 display 只接受 TS `toProtocolToolCallDisplay` 白名单中的 kind。
    pub fn finish_tool_row(&mut self, turn: &str, call_id: &str, done: ToolResult, now: u64) -> Option<Value> {
        let row = self.rows.iter_mut().find(|r| r["turnId"] == turn && r["toolCallId"] == call_id)?;
        if done.denied {
            row["status"] = "cancelled".into();
        } else {
            row["status"] = if done.failed { "error" } else { "success" }.into();
            row["endedAt"] = now.into();
            row["output"] = json!({"text": done.result});
            if let Some(display) = done.display {
                if display["kind"] != "node_repl_images" {
                    row["output"]["display"] = display.clone();
                }
                if super::tool_display::row_display_kind(&display) {
                    row["display"] = display;
                }
            }
            if done.failed {
                row["error"] = json!({"code":"tool_execution_failed","message":"Tool execution failed"});
            }
        }
        row.as_object_mut()?.remove("approvalInteractionId");
        Some(row.clone())
    }
    /// TS appendBrowserTurnScreenshot 的工具行：`mcp__node_repl__js`，input `{source: browser_turn_end}`，空输出，图片卡。
    pub fn turn_screenshot_row(&mut self, turn: &str, response: Option<String>, display: Value, id: &str, now: u64) -> Value {
        let input = json!({"source": "browser_turn_end"});
        let call = json!({"id": format!("tool_{id}"), "function": {"name": "mcp__node_repl__js", "arguments": input.to_string()}});
        let mut row = self.tool_call_row(turn, &call, response, now);
        row["status"] = "success".into();
        row["endedAt"] = now.into();
        row["output"] = json!({"text": ""});
        row["display"] = display;
        self.rows.push(row.clone());
        row
    }
    /// 运行中的工具行；带发出调用的模型响应 id 与结构化 input（TS toolCall，rust-row-projection.md）。
    pub fn tool_call_row(&mut self, turn: &str, call: &Value, response_id: Option<String>, now: u64) -> Value {
        let mut row = self.row("toolCall", turn, call["id"].as_str().unwrap_or_default(), now);
        row["toolCallId"] = call["id"].clone();
        row["toolName"] = call["function"]["name"].clone();
        row["inputText"] = call["function"]["arguments"].clone();
        row["assistantResponseId"] = response_id.into();
        if let Some(input) = call["function"]["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok()) {
            row["input"] = input;
        }
        row["status"] = "running".into();
        row["startedAt"] = now.into();
        row
    }
    pub fn close_unfinished_tools(&mut self) {
        let mut unresolved = std::collections::BTreeMap::new();
        for message in &self.messages {
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    unresolved.insert(call["id"].as_str().unwrap_or("").to_owned(), ());
                }
            }
            if let Some(id) = message["tool_call_id"].as_str() {
                unresolved.remove(id);
            }
        }
        for (id, _) in unresolved {
            let answered = self.rows.iter().find(|r| {
                r["toolCallId"] == id
                    && matches!(
                        r["toolName"].as_str(),
                        Some("AskUserQuestion" | "TodoRead" | "TodoWrite")
                    )
                    && matches!(r["status"].as_str(), Some("success" | "error"))
                    && r["output"]["text"].is_string()
            });
            let content=answered.map(|r|r["output"]["text"].clone()).unwrap_or_else(||json!("Interrupted; execution outcome is unknown. Do not assume this action was not performed."));
            let failed = answered.is_some_and(|r| r["status"] == "error");
            self.append_message(json!({"role":"tool","tool_call_id":id,"content":content,"_zcode_tool_failed":failed}));
        }
    }
    pub fn finish_rows(&mut self, outcome: &str, now: u64) {
        let history_rounds = history_rounds(&self.rows);
        for row in &mut self.rows {
            if row["kind"] == "timelineMarker"
                && row["marker"]["type"] == "goalVerify"
                && row["marker"]["outcome"] == "running"
            {
                row["marker"]["outcome"] = "failed".into();
                row["marker"]["detail"] = "Goal verification interrupted".into();
                if let Some(goal) = &mut self.goal {
                    goal.verifications.push(json!({"iteration":row["marker"]["iteration"],"outcome":"failed","at":now,"anchorRowId":row["rowId"],"reason":"Goal verification interrupted"}));
                }
            }
            if row["kind"] == "timelineMarker"
                && row["marker"]["type"] == "compact"
                && row["marker"]["status"] == "running"
            {
                row["marker"]["status"] = if outcome == "interrupted" {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
            }
            if row["state"] == "streaming" {
                row["state"] = if outcome == "success" {
                    "complete"
                } else {
                    "interrupted"
                }
                .into();
            }
            if row["kind"] == "turnHeader" && row["state"] == "running" {
                row["state"] = match outcome {
                    "success" => "completedSuccess",
                    "interrupted" => "completedInterrupted",
                    _ => "failed",
                }
                .into();
                row["endedAt"] = now.into();
                // TS upsertTurnHeader：agent 轮写工时；controlOnly 没有 Agent 工时（UI 会把 0 秒显示成「已工作 1 秒」）。
                if row["executionKind"] != "controlOnly" {
                    row["activeMs"] = now.saturating_sub(row["startedAt"].as_u64().unwrap_or(now)).into();
                }
                let rounds = history_rounds.get(row["turnId"].as_str().unwrap_or("")).copied().unwrap_or(0);
                // controlOnly 轮只有产生历史（如 compact 摘要）时才有该字段（TS payload 缺省）。
                if row["executionKind"] != "controlOnly" || rounds > 0 {
                    row["historyRoundCount"] = rounds.into();
                }
            }
            // 子代理由独立 child 生命周期收口；父回复结束不能把仍在运行的后台 child 标成取消。
            if row["kind"] != "subagent"
                && matches!(row["status"].as_str(), Some("running" | "pendingApproval"))
            {
                row["status"] = "cancelled".into();
                row.as_object_mut().unwrap().remove("approvalInteractionId");
            }
        }
    }
}

/// 行基础字段（docs/specs/rust-row-projection.md）：TS 行都带 `visibility`；turnHeader 默认 `executionKind: agent`，
/// controlOnly 轮由创建方覆盖。
pub fn row_base(mut row: Value) -> Value {
    row["visibility"] = "visible".into();
    if row["kind"] == "turnHeader" {
        row["executionKind"] = "agent".into();
    }
    row
}

/// TS `historyRoundCount`：本轮写入历史的模型轮次（每个模型响应 1 次）与成功的 compact 摘要（各 1 次）。
fn history_rounds(rows: &[Value]) -> std::collections::BTreeMap<String, u64> {
    let mut responses = std::collections::BTreeSet::new();
    let mut counts = std::collections::BTreeMap::new();
    for row in rows {
        let turn = row["turnId"].as_str().unwrap_or("").to_owned();
        let response = row["assistantResponseId"].as_str();
        let compact = row["marker"]["type"] == "compact" && row["marker"]["status"] == "success";
        let fresh = response.is_some_and(|r| responses.insert((turn.clone(), r.to_owned())));
        if fresh || compact {
            *counts.entry(turn).or_insert(0) += 1;
        }
    }
    counts
}
