//! `computer-use/operation-event`（TS bootstrap/escode-protocol/computer-use-operation-event.ts）：Desktop 的
//! Computer Use 顶部提示依赖的无内容生命周期元数据——回合开始 / 完成 / 失败、工具排定 / 开始（TS 的 session-closed
//! 来自 SessionEnded，deleteSession 路径上 Node 并不发出，这里同样不发）。
//! 由遥测事实派生（同一时刻），工具排定额外带「本 cell 是否在用 CUA」的布尔事实。
use super::Engine;
use serde_json::{Value, json};

impl Engine {
    fn cua_notify(&mut self, session: &str, kind: &str, fields: Value) {
        let seq = self
            .telemetry
            .cua_seq
            .entry(session.to_owned())
            .or_default();
        let mut params = json!({
            "eventId": self.clock.id(), "sequenceNumber": *seq, "sessionId": session,
            "timestamp": self.clock.now(), "kind": kind,
        });
        *seq += 1;
        for (key, value) in fields.as_object().into_iter().flatten() {
            if !value.is_null() {
                params[key] = value.clone();
            }
        }
        self.outbox
            .push(json!({ "method": "computer-use/operation-event", "params": params }));
    }

    /// 遥测事实 → 生命周期通知（工具排定由 `cua_tool_scheduled` 发出，它需要模型入参）。
    pub(super) fn cua_from_fact(&mut self, fact: &Value) {
        let session = fact["sessionId"].as_str().unwrap_or_default().to_owned();
        let turn = fact["turnId"].clone();
        let kind = match (
            fact["kind"].as_str(),
            fact["phase"].as_str(),
            fact["status"].as_str(),
        ) {
            (Some("turn.started"), ..) if turn.is_string() => "turn-started",
            // TS：取消属于正常结束，复用 TurnComplete。
            (Some("turn.terminal"), _, Some("failed")) if turn.is_string() => "turn-failed",
            (Some("turn.terminal"), ..) if turn.is_string() => "turn-completed",
            (Some("tool.lifecycle"), Some("started"), _) => {
                let fields = json!({"turnId": turn, "toolCallId": fact["toolCallId"], "toolName": fact["toolName"]});
                return self.cua_notify(&session, "tool-started", fields);
            }
            _ => return,
        };
        self.cua_notify(&session, kind, json!({ "turnId": turn }));
    }

    pub(super) fn cua_tool_scheduled(&mut self, session: &str, turn: &str, call: &Value) {
        let name = call["function"]["name"].as_str().unwrap_or_default();
        // TS usesComputerUse：node_repl cell 必含的 CUA 引导语句是唯一锚点。
        let uses = name == "mcp__node_repl__js"
            && serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap_or(""))
                .ok()
                .and_then(|input| {
                    input["code"]
                        .as_str()
                        .map(|c| c.contains("setupComputerUseRuntime"))
                })
                .unwrap_or(false);
        let mut fields = json!({"turnId": turn, "toolCallId": call["id"], "toolName": name});
        if uses {
            fields["computerUse"] = true.into();
        }
        self.cua_notify(session, "tool-scheduled", fields);
    }
}
