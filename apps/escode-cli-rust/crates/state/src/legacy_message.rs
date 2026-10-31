//! 冷导入的 assistant 消息投影（从 legacy_sessions.rs 拆出，控制单文件规模）。
//!
//! Node 冷恢复按 part 顺序创建行：text/reasoning 与 tool 交错；轮次事实来自 transcript
//! anchor 或独立 assistant 条目计数。

use crate::domain::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) struct Assistant<'a> {
    pub mid: &'a str,
    pub message: &'a Value,
    pub parts: &'a [(String, Value)],
    pub text: &'a str,
    pub now: u64,
    pub turn: &'a mut String,
    pub interrupted: &'a mut bool,
    pub rounds: &'a mut u64,
    pub ended_at: &'a mut u64,
    pub message_rows: &'a mut Vec<usize>,
    /// pending/running 工具行下标；Node 在 TurnComplete(cancelled) 时统一补 endedAt。
    pub open_tools: &'a mut Vec<usize>,
    /// part id → Node `stringifyToolInput` 文本（保留 TS 持久化的键序）。
    pub input_texts: &'a BTreeMap<String, String>,
    pub compact_boundary: &'a Value,
    pub legacy_tail: Option<&'a str>,
    pub positions: &'a BTreeMap<String, (usize, usize)>,
}

impl Assistant<'_> {
    pub(super) fn project(self, session: &mut Session) {
        let message = self.message;
        // Node collectTurnOutput 在轮次统计与行合成之前整条跳过 providerContextOnly
        // assistant（summary/compact_summary/model-only）。此前先累加 round，compact 摘要轮
        // 的 historyRoundCount 因此比 Node 多 1，摘要的 endedAt 也会错误推进轮次跨度。
        if provider_context_only(message) {
            if message["summary"] == true {
                session.context.offset = self.compact_boundary["preservedSegment"]["headMessageId"]
                    .as_str()
                    .or(self.legacy_tail)
                    .and_then(|id| self.positions.get(id).map(|p| p.0))
                    .or_else(|| {
                        self.compact_boundary["lastSummarizedMessageId"]
                            .as_str()
                            .and_then(|id| self.positions.get(id).map(|p| p.1))
                    })
                    .unwrap_or(session.messages.len());
                session.context.summary = Some(self.text.into());
            }
            return;
        }
        if let Some(end) = super::legacy_projection::message_end_at(message, self.parts) {
            *self.ended_at = (*self.ended_at).max(end);
        }
        // Node transcript 在最终 anchor 固化精确轮次；旧 transcript 才按 assistant 条目累加。
        *self.rounds = message["anchor"]["historyRoundCount"]
            .as_u64()
            .unwrap_or(*self.rounds + 1);
        if self.turn.is_empty() {
            *self.turn = message["parentID"].as_str().unwrap_or(self.mid).into();
        }
        let unfinished = message["time"]["completed"].is_null();
        *self.interrupted |= unfinished;
        let reasoning = self
            .parts
            .iter()
            .filter(|(_, p)| p["type"] == "reasoning")
            .filter_map(|(_, p)| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut declared = self
            .parts
            .iter()
            .filter(|(_, p)| p["type"] == "tool")
            .collect::<Vec<_>>();
        declared.sort_by_key(|(_, p)| p["declarationIndex"].as_u64().unwrap_or(u64::MAX));
        let mut canonical = json!({"role":"assistant","content":self.text,"reasoning_content":reasoning,"_escode_origin":{"provider":message["providerId"].as_str().unwrap_or("unknown"),"model":message["modelId"].as_str().unwrap_or("unknown")}});
        super::legacy_projection::reasoning(&mut canonical, self.parts);
        if !declared.is_empty() {
            canonical["tool_calls"]=declared.iter().map(|(_,p)|json!({"id":p["callID"],"type":"function","function":{"name":p["tool"],"arguments":p["state"]["input"].to_string()}})).collect();
        }
        let visible = message["semantics"]["providerVisibility"] != "hidden";
        if visible {
            session.messages.push(canonical);
        }
        // Node 按 part 顺序创建行（tool 与 text 交错）；此前把所有 tool 行追加到最后，
        // 真实数据的 [tool, text] part 顺序因此行列与 Node 相反。
        for (pid, p) in self.parts {
            match p["type"].as_str() {
                // 与 Node 冷恢复一致：空文本的 text/reasoning part 不成行（部分供应商只在
                // metadata 保存加密推理，text 为空）；模型上下文仍由 canonical 消息承载。
                Some("text" | "reasoning")
                    if p["ignored"] != true
                        && p["text"].as_str().is_some_and(|t| !t.is_empty()) =>
                {
                    let kind = if p["type"] == "reasoning" {
                        "reasoning"
                    } else {
                        "assistantText"
                    };
                    let mut row = session.row(kind, self.turn, pid, self.now);
                    row["text"] = p["text"].clone();
                    row["assistantResponseId"] = self.mid.into();
                    // Node 冷恢复用 ModelComplete 收口流式行，正文行始终 complete；
                    // 中断只体现在 turnHeader/session phase 与 tool 行状态。
                    row["state"] = "complete".into();
                    session.rows.push(row);
                    self.message_rows.push(session.rows.len() - 1);
                }
                Some("tool") => {
                    let state = &p["state"];
                    let status = state["status"].as_str().unwrap_or("pending");
                    let output = match status {
                        "completed" => state["output"].as_str().unwrap_or(""),
                        "error" => state["error"].as_str().unwrap_or("Tool failed"),
                        _ => {
                            "Interrupted; execution outcome is unknown. Inspect effects before deciding whether to retry."
                        }
                    };
                    *self.interrupted |= !matches!(status, "completed" | "error");
                    if message["semantics"]["providerVisibility"] != "hidden" {
                        session.messages.push(json!({"role":"tool","tool_call_id":p["callID"],"content":output,"_escode_tool_failed":status!="completed"}));
                    }
                    // Node toolCall entityId 即 toolCallId；结构化 input 与起止时间来自 part state。
                    let mut row = session.row(
                        "toolCall",
                        self.turn,
                        p["callID"].as_str().unwrap_or(pid),
                        self.now,
                    );
                    row["assistantResponseId"] = self.mid.into();
                    row["toolCallId"] = p["callID"].clone();
                    row["toolName"] = p["tool"].clone();
                    row["inputText"] = self
                        .input_texts
                        .get(pid)
                        .cloned()
                        .unwrap_or_else(|| match state.get("input") {
                            Some(value) if !value.is_null() => value.to_string(),
                            _ => "{}".into(),
                        })
                        .into();
                    row["input"] = state["input"].clone();
                    if let Some(start) = state["time"]["start"].as_u64() {
                        row["startedAt"] = start.into();
                    }
                    if let Some(end) = state["time"]["end"].as_u64() {
                        row["endedAt"] = end.into();
                    }
                    row["status"] = match status {
                        "completed" => "success",
                        "error" => "error",
                        _ => "cancelled",
                    }
                    .into();
                    let terminal = matches!(status, "completed" | "error");
                    if terminal {
                        // Node 只在 ToolCallResult 终态事件写 output；pending/running 由
                        // TurnComplete(cancelled) 收口时保持无 output（此前伪造了提示文本）。
                        row["output"] = json!({"text":output});
                    }
                    if status == "error" {
                        row["error"] = json!({"code":"tool.failed","message":output});
                    }
                    let index = session.rows.len();
                    session.rows.push(row);
                    if !terminal {
                        self.open_tools.push(index);
                    }
                }
                _ => {}
            }
        }
        for (key, target) in [("input", "inputTokens"), ("output", "outputTokens")] {
            let old = session.usage["cumulative"][target].as_u64().unwrap_or(0);
            session.usage["cumulative"][target] =
                (old + message["tokens"][key].as_u64().unwrap_or(0)).into();
        }
    }
}

/// Node `isProviderContextOnlyAssistant`：冷恢复在轮次统计和行合成前整条跳过。
fn provider_context_only(message: &Value) -> bool {
    let semantics = &message["semantics"];
    if message.get("summary").is_some() || semantics["kind"] == "compact_summary" {
        return true;
    }
    // 正常 assistant 同时进入 provider context 与可见 transcript；须先于
    // providerVisibility 判定，否则晚订阅的冷恢复会丢掉正文行。
    if semantics["kind"] == "assistant_response"
        && semantics["uiVisibility"] == "visible"
        && semantics["transcriptVisibility"] == "visible"
    {
        return false;
    }
    semantics["providerVisibility"] == "visible" || message["visibility"] == "model-only"
}

/// Node `stringifyToolInput(input ?? {})`。Value 重新序列化会按字典序重排键，这里从
/// part 原始 JSON 文本抽取 `state.input`，只压缩字符串外的空白，保持 TS 写入的键序。
pub(super) fn tool_input_text(data: &str) -> String {
    let Ok(part) = serde_json::from_str::<BTreeMap<String, Box<serde_json::value::RawValue>>>(data)
    else {
        return "{}".into();
    };
    let Some(state) = part.get("state") else {
        return "{}".into();
    };
    let Ok(mut state) =
        serde_json::from_str::<BTreeMap<String, Box<serde_json::value::RawValue>>>(state.get())
    else {
        return "{}".into();
    };
    let Some(input) = state.remove("input") else {
        return "{}".into();
    };
    if input.get() == "null" {
        "{}".into()
    } else {
        compact_json(input.get())
    }
}

fn compact_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let (mut in_string, mut escaped) = (false, false);
    for ch in raw.chars() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            out.push(ch);
        } else if !ch.is_whitespace() {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_input_text_keeps_stored_key_order_and_compacts_whitespace() {
        let data = r#"{"type":"tool","state":{"status":"running","input": {"file_path":"v.ts", "old_string":"a", "new_string":"b"}}}"#;
        assert_eq!(
            tool_input_text(data),
            r#"{"file_path":"v.ts","old_string":"a","new_string":"b"}"#
        );
    }

    #[test]
    fn tool_input_text_falls_back_to_an_empty_object() {
        for data in [
            r#"{"type":"tool","state":{"status":"pending"}}"#,
            r#"{"type":"tool","state":{"input":null}}"#,
            "not json",
        ] {
            assert_eq!(tool_input_text(data), "{}");
        }
    }

    #[test]
    fn provider_context_only_matches_node_policy_for_assistants() {
        assert!(provider_context_only(
            &json!({"role":"assistant","summary":true})
        ));
        assert!(provider_context_only(
            &json!({"role":"assistant","summary":false})
        ));
        assert!(provider_context_only(
            &json!({"role":"assistant","semantics":{"kind":"compact_summary"}})
        ));
        assert!(!provider_context_only(
            &json!({"role":"assistant","semantics":{
            "kind":"assistant_response","uiVisibility":"visible",
            "transcriptVisibility":"visible","providerVisibility":"visible"}})
        ));
        assert!(provider_context_only(
            &json!({"role":"assistant","semantics":{"providerVisibility":"visible"}})
        ));
        assert!(provider_context_only(
            &json!({"role":"assistant","visibility":"model-only"})
        ));
        assert!(!provider_context_only(&json!({"role":"assistant"})));
    }
}
