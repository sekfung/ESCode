//! Claude 导入会话的重导入替换（docs/specs/rust-legacy-session-methods.md「重导入与历史替换」）：
//! 删除旧导入段并按新历史重建，保留用户续聊；所有按行/消息索引保存的事实同步平移。
use super::{
    claude_import::{self, ClaudeHistory},
    session::Session,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn is_import_marker(session: &Session, turn: &str) -> bool {
    turn.starts_with(&format!("{}_import_", session.id))
        || turn.starts_with("msg_import_")
        || turn.starts_with("msg_claude-import-")
}

/// 用新导入历史替换旧导入段，返回 `(旧导入行数, 新导入行数)`。
///
/// Node `removePreviousImportedSessionHistory` 先删旧导入消息，再 `saveMessage` 追加新消息；
/// `sessionStore.messages()` 按 message `sequence` 读取，所以修复后的顺序是「保留的续聊 → 新导入段」。
/// Rust 保持同一顺序：按 canonical 标记删除旧导入消息（标记缺席时退回开头的 legacy 导入段），
/// 保留续聊行及其 `rowId`（检查点/目标锚点无需重映射），再把新导入段追加到末尾。
pub fn replace(session: &mut Session, history: &ClaudeHistory, created: u64) -> (usize, usize) {
    let rows = marked_rows(session);
    let mut messages = marked_messages(session);
    if messages.is_empty() && !rows.is_empty() {
        // 旧版本导入的 canonical 消息没有标记；那时的导入段必定在会话开头。
        messages = (0..legacy_message_count(session, &rows).min(session.messages.len())).collect();
    }
    let rows_removed = remove(&mut session.rows, &rows);
    let messages_removed = remove(&mut session.messages, &messages);
    session.context.offset = remap(session.context.offset, &messages_removed);
    shift_facts(session, &rows_removed, &messages_removed);
    session.context_tokens = None;
    let preserved = session.rows.len();
    if preserved == 0 {
        session.row_highwater = 0;
    }
    claude_import::build(session, history, created);
    let new_rows = session.rows.len() - preserved;
    session.history_rewrite = true;
    session.saved_rows = 0;
    session.saved_messages = 0;
    session.saved_inputs = 0;
    session.saved_responses = 0;
    (rows.len(), new_rows)
}

fn marked_rows(session: &Session) -> Vec<usize> {
    (0..session.rows.len())
        .filter(|index| {
            session.rows[*index]["turnId"]
                .as_str()
                .is_some_and(|turn| is_import_marker(session, turn))
        })
        .collect()
}

fn marked_messages(session: &Session) -> Vec<usize> {
    (0..session.messages.len())
        .filter(|index| {
            session.messages[*index]["_escode_import_source"] == claude_import::IMPORT_SOURCE
        })
        .collect()
}

/// 旧导入消息数：每条 userInput 一行，assistant 的 text/reasoning/tool 行共享一条 canonical 消息。
fn legacy_message_count(session: &Session, rows: &[usize]) -> usize {
    let mut users = 0;
    let mut responses = BTreeSet::new();
    for index in rows {
        let row = &session.rows[*index];
        if row["kind"] == "userInput" {
            users += 1;
        } else if let Some(id) = row["assistantResponseId"].as_str() {
            responses.insert(id);
        }
    }
    users + responses.len()
}

fn remove(values: &mut Vec<Value>, indices: &[usize]) -> BTreeSet<usize> {
    let removed: BTreeSet<usize> = indices.iter().copied().collect();
    let mut index = 0;
    values.retain(|_| {
        let keep = !removed.contains(&index);
        index += 1;
        keep
    });
    removed
}

fn remap(index: usize, removed: &BTreeSet<usize>) -> usize {
    index - removed.range(..index).count()
}

fn shift_facts(
    session: &mut Session,
    rows_removed: &BTreeSet<usize>,
    messages_removed: &BTreeSet<usize>,
) {
    for boundary in &mut session.history.inputs {
        boundary.row = remap(boundary.row, rows_removed);
        boundary.user_row = remap(boundary.user_row, rows_removed);
        boundary.message = remap(boundary.message, messages_removed);
        boundary.state.context.offset = remap(boundary.state.context.offset, messages_removed);
    }
    for boundary in &mut session.history.responses {
        boundary.row = remap(boundary.row, rows_removed);
        boundary.message = remap(boundary.message, messages_removed);
        boundary.state.context.offset = remap(boundary.state.context.offset, messages_removed);
    }
    session.history.action_rows.clear();
    for record in &mut session.legacy_topic_records {
        if let Some(anchor) = record["rowAnchor"].as_u64() {
            record["rowAnchor"] = remap(anchor as usize, rows_removed).into();
        }
    }
    if let Some((at, _)) = &mut session.shell_notice {
        *at = remap(*at, messages_removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{InputBoundary, ResponseBoundary, State};
    use crate::{
        claude_import::{ClaudeHistory, ClaudeMessage},
        file_checkpoint::FileCheckpoint,
        session::Session,
    };
    use serde_json::json;

    fn history(messages: &[(&str, &str, u64)]) -> ClaudeHistory {
        ClaudeHistory {
            source: "claudeCode".into(),
            title: None,
            created_at: Some(1000),
            updated_at: None,
            messages: messages
                .iter()
                .map(|(role, content, timestamp)| ClaudeMessage {
                    role: (*role).into(),
                    content: (*content).into(),
                    timestamp: Some(*timestamp),
                })
                .collect(),
        }
    }

    fn session() -> Session {
        Session::new(
            "claude-import-x".into(),
            "w".into(),
            "p".into(),
            "m".into(),
            "low".into(),
            "epoch".into(),
            1000,
        )
    }

    fn follow_up(s: &mut Session) {
        let turn = "turn-real".to_owned();
        let mut header = s.row("turnHeader", &turn, &turn, 9000);
        header["origin"] = "userInput".into();
        header["state"] = "completedSuccess".into();
        s.rows.push(header);
        let mut user = s.row("userInput", &turn, &turn, 9000);
        user["text"] = "follow".into();
        user["origin"] = "realUser".into();
        s.rows.push(user);
        s.append_message(json!({"role":"user","content":"follow"}));
        s.history.inputs.push(InputBoundary {
            entity: turn.clone(),
            turn: turn.clone(),
            row: s.rows.len() - 2,
            user_row: s.rows.len() - 1,
            message: s.messages.len(),
            state: State::capture(s),
            kind: "sendText".into(),
            payload: json!({"text":"follow"}),
        });
        let mut reply = s.row("assistantText", &turn, "real-a", 9100);
        reply["text"] = "ok".into();
        reply["assistantResponseId"] = "real-a".into();
        reply["state"] = "complete".into();
        s.rows.push(reply);
        s.append_message(json!({"role":"assistant","content":"ok"}));
        s.history.responses.push(ResponseBoundary {
            entity: "real-a".into(),
            turn,
            row: s.rows.len() - 1,
            message: s.messages.len(),
            state: State::capture(s),
        });
        let row = s.rows.last().unwrap()["rowId"].as_u64().unwrap();
        s.file_checkpoints.push(FileCheckpoint {
            id: "cp".into(),
            path: "p".into(),
            tool: "write".into(),
            before: None,
            after: "x".into(),
            mode: None,
            row,
            restored: false,
        });
    }

    #[test]
    fn replaces_the_imported_prefix_after_preserved_follow_up() {
        let mut s = session();
        claude_import::build(
            &mut s,
            &history(&[("user", "q1", 1000), ("assistant", "a1", 1100)]),
            1000,
        );
        follow_up(&mut s);
        let checkpoint = s.file_checkpoints[0].row;
        let (old_rows, new_rows) = replace(
            &mut s,
            &history(&[
                ("user", "q1", 1000),
                ("assistant", "a1", 1100),
                ("user", "q2", 1200),
                ("assistant", "a2", 1300),
            ]),
            1000,
        );
        assert_eq!((old_rows, new_rows), (3, 6));
        // Node 按 message sequence 读取：删除旧导入消息后，新导入段追加在续聊之后。
        let texts: Vec<_> = s
            .rows
            .iter()
            .filter(|r| matches!(r["kind"].as_str(), Some("userInput" | "assistantText")))
            .map(|r| r["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["follow", "ok", "q1", "a1", "q2", "a2"]);
        let roles: Vec<_> = s
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            [
                "user",
                "assistant",
                "user",
                "assistant",
                "user",
                "assistant"
            ]
        );
        // 续聊行保留原 rowId；新导入段接在其后。
        let ids: Vec<_> = s
            .rows
            .iter()
            .map(|r| r["rowId"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, [4, 5, 6, 7, 8, 9, 10, 11, 12]);
        // 续聊边界随删除的导入段前移，检查点 rowId 不变。
        assert_eq!(s.history.inputs[0].row, 0);
        assert_eq!(s.history.inputs[0].user_row, 1);
        assert_eq!(s.history.inputs[0].message, 1);
        assert_eq!(s.history.responses[0].row, 2);
        assert_eq!(s.history.responses[0].message, 2);
        assert_eq!(s.file_checkpoints[0].row, checkpoint);
        assert!(s.history_rewrite);
    }

    #[test]
    fn appends_import_block_when_session_has_no_imported_prefix() {
        let mut s = session();
        follow_up(&mut s);
        let (old_rows, new_rows) = replace(&mut s, &history(&[("user", "q1", 1000)]), 1000);
        assert_eq!((old_rows, new_rows), (0, 2));
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
        assert_eq!(s.history.inputs[0].user_row, 1);
        assert_eq!(s.history.responses[0].row, 2);
        assert_eq!(s.file_checkpoints[0].row, 3);
    }

    #[test]
    fn repeated_reimport_keeps_a_single_imported_block() {
        let mut s = session();
        claude_import::build(
            &mut s,
            &history(&[("user", "q1", 1000), ("assistant", "a1", 1100)]),
            1000,
        );
        follow_up(&mut s);
        let grown = history(&[
            ("user", "q1", 1000),
            ("assistant", "a1", 1100),
            ("user", "q2", 1200),
            ("assistant", "a2", 1300),
        ]);
        replace(&mut s, &grown, 1000);
        let rows = s.rows.len();
        let roles: Vec<String> = s
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap().to_owned())
            .collect();
        let (removed, added) = replace(&mut s, &grown, 1000);
        assert_eq!((removed, added), (6, 6));
        assert_eq!(s.rows.len(), rows);
        assert_eq!(
            s.messages
                .iter()
                .map(|m| m["role"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            roles
        );
        let texts: Vec<_> = s
            .rows
            .iter()
            .filter(|r| matches!(r["kind"].as_str(), Some("userInput" | "assistantText")))
            .map(|r| r["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["follow", "ok", "q1", "a1", "q2", "a2"]);
    }
}
