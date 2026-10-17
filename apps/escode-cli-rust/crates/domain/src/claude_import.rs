//! Claude Code 历史导入的纯投影（TS `persistImportedSessionHistory` claudeCode 分支；
//! docs/specs/rust-legacy-session-methods.md）：消息数组 → 行 + canonical 消息，与 legacy 导入器同形。
use super::{session::Session, shared_import::ClaudeHistory};
use serde_json::json;

/// 会话时间：`createdAt ?? 首条时间`，`updatedAt` 不早于 `createdAt`（TS 同）。
pub fn times(history: &ClaudeHistory, now: u64) -> (u64, u64) {
    let created = history
        .created_at
        .or_else(|| history.messages.first().and_then(|m| m.timestamp))
        .unwrap_or(now);
    let updated = history
        .updated_at
        .or_else(|| history.messages.last().and_then(|m| m.timestamp))
        .unwrap_or(created);
    (created, updated.max(created))
}

pub fn build(session: &mut Session, history: &ClaudeHistory, created: u64) {
    let mut turn = String::new();
    // TS 以数组顺序为准，写库前把时间收敛为严格递增，避免 assistant 排到 user 之前。
    let mut last = created.saturating_sub(1);
    for (index, message) in history.messages.iter().enumerate() {
        let raw = message.timestamp.unwrap_or(created + index as u64);
        let at = if raw > last { raw } else { last + 1 };
        last = at;
        let entity = format!("{}_import_{index}", session.id);
        let user = message.role == "user";
        if user || turn.is_empty() {
            turn = entity.clone();
            let mut header = session.row("turnHeader", &turn, &turn, at);
            header["origin"] = "userInput".into();
            header["state"] = "completedSuccess".into();
            header["startedAt"] = at.into();
            header["endedAt"] = at.into();
            session.rows.push(header);
        }
        if user {
            let mut row = session.row("userInput", &turn, &entity, at);
            row["text"] = message.content.clone().into();
            row["origin"] = "realUser".into();
            session.rows.push(row);
            session.append_message(json!({"role":"user","content":message.content}));
        } else {
            let mut row = session.row("assistantText", &turn, &format!("{entity}_text"), at);
            row["text"] = message.content.clone().into();
            row["assistantResponseId"] = entity.clone().into();
            row["state"] = "complete".into();
            session.rows.push(row);
            session.append_message(json!({"role":"assistant","content":message.content,
                "_escode_origin":{"provider":"unknown","model":"unknown"}}));
        }
    }
    session.updated_at = session.updated_at.max(last);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_import::{ClaudeHistory, ClaudeMessage};

    fn history(messages: &[(&str, &str, Option<u64>)]) -> ClaudeHistory {
        ClaudeHistory {
            source: "claudeCode".into(),
            title: None,
            created_at: None,
            updated_at: None,
            messages: messages
                .iter()
                .map(|(role, content, timestamp)| ClaudeMessage {
                    role: (*role).into(),
                    content: (*content).into(),
                    timestamp: *timestamp,
                })
                .collect(),
        }
    }

    #[test]
    fn messages_become_turns_rows_and_canonical_history_in_order() {
        // assistant 时间早于 user：按数组顺序收敛为递增。
        let h = history(&[
            ("user", "q1", Some(200)),
            ("assistant", "a1", Some(100)),
            ("user", "q2", None),
        ]);
        let (created, updated) = times(&h, 9);
        assert_eq!((created, updated), (200, 200));
        let mut s = Session::new(
            "claude-import-x".into(),
            "w".into(),
            "".into(),
            "".into(),
            "".into(),
            "t".into(),
            created,
        );
        build(&mut s, &h, created);
        let kinds: Vec<_> = s.rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "turnHeader",
                "userInput",
                "assistantText",
                "turnHeader",
                "userInput"
            ]
        );
        let times: Vec<_> = s
            .rows
            .iter()
            .map(|r| r["createdAt"].as_u64().unwrap())
            .collect();
        assert_eq!(times, [200, 200, 201, 202, 202]);
        assert_eq!(s.rows[2]["turnId"], s.rows[1]["turnId"]);
        let roles: Vec<_> = s
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
        assert_eq!(s.messages[1]["content"], "a1");
    }

    #[test]
    fn leading_assistant_gets_its_own_turn_header() {
        let h = history(&[("assistant", "hello", Some(5))]);
        let mut s = Session::new(
            "s".into(),
            "w".into(),
            "".into(),
            "".into(),
            "".into(),
            "t".into(),
            5,
        );
        build(&mut s, &h, 5);
        assert_eq!(s.rows[0]["kind"], "turnHeader");
        assert_eq!(s.rows[1]["kind"], "assistantText");
    }
}
