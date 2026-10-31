//! 冷导入收口后的行派生事实：命令 actions 与 createdAtSeq 分组。
//! 两者都对齐 Node 冷恢复（synthesizeEventsFromMessages → ProductProjection）的最终行态。

use crate::domain::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Node 冷恢复的 createdAtSeq 是合成事件号：同一个 TurnStarted 产出的模型切换标记、
/// turnHeader 与 userInput 共享事件号，其余行各由独立事件（流式开始/工具调度/时间线）创建。
/// 差分投影按首次出现排名比较，所以只需保持同样的相等分组与出现顺序。
pub(super) fn imported_sequences(session: &mut Session) {
    let mut seq = 0u64;
    let mut turn = 0u64;
    let mut pending_markers: Vec<usize> = Vec::new();
    for index in 0..session.rows.len() {
        match session.rows[index]["kind"].as_str() {
            Some("turnHeader") => {
                seq += 1;
                turn = seq;
                for marker in pending_markers.drain(..) {
                    session.rows[marker]["createdAtSeq"] = seq.into();
                }
                session.rows[index]["createdAtSeq"] = seq.into();
            }
            Some("userInput") => {
                if turn == 0 {
                    seq += 1;
                    turn = seq;
                }
                session.rows[index]["createdAtSeq"] = turn.into();
            }
            Some("timelineMarker") if session.rows[index]["marker"]["type"] == "modelChange" => {
                pending_markers.push(index);
            }
            _ => {
                seq += 1;
                session.rows[index]["createdAtSeq"] = seq.into();
            }
        }
    }
    // 会话末尾没有后继 turnHeader 的模型标记没有可并入的轮次，按独立事件收口。
    for marker in pending_markers {
        seq += 1;
        session.rows[marker]["createdAtSeq"] = seq.into();
    }
}

/// 冷导入 actions 对齐 Node `materializeCommandRowActions` 的收口结果：
/// 每轮最后一条 assistantText 可 fork；只有全局最新 realUser 输入可编辑；
/// 只有全局最新 assistantText 且同轮存在可解析 realUser 来源时才可 retry。
/// `bound` 是拥有持久 transcript message 身份的行下标（Node `messageIdByRowId`）。
pub(super) fn imported_actions(session: &mut Session, bound: &[usize]) {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut latest_by_turn: BTreeMap<String, usize> = BTreeMap::new();
    for (index, row) in session.rows.iter().enumerate() {
        let Some(turn) = row["turnId"].as_str() else {
            continue;
        };
        match row["kind"].as_str() {
            Some("turnHeader") => {
                headers.insert(turn.into(), row["state"].as_str().unwrap_or("").into());
            }
            Some("assistantText") => {
                latest_by_turn.insert(turn.into(), index);
            }
            _ => {}
        }
    }
    // Node 先锁定最新 realUser 行，再用 message 身份做资格校验；不能因最新行未绑定就回退到更早的行。
    let latest_editable = session
        .rows
        .iter()
        .rposition(|row| row["kind"] == "userInput" && row["origin"] == "realUser")
        .filter(|index| bound.contains(index));
    let latest_assistant = session
        .rows
        .iter()
        .rposition(|row| row["kind"] == "assistantText");
    let retryable = latest_assistant.filter(|index| {
        let row = &session.rows[*index];
        let turn = row["turnId"].as_str().unwrap_or("");
        row["state"] == "complete"
            && bound.contains(index)
            && headers.get(turn).is_some_and(|state| state != "running")
            && session.rows.iter().enumerate().any(|(user, candidate)| {
                candidate["kind"] == "userInput"
                    && candidate["origin"] == "realUser"
                    && candidate["turnId"].as_str() == Some(turn)
                    && bound.contains(&user)
            })
    });
    let mut wanted = Vec::new();
    for (index, row) in session.rows.iter().enumerate() {
        match row["kind"].as_str() {
            Some("userInput") => wanted.push((
                index,
                (Some(index) == latest_editable)
                    .then(|| json!({"canEdit":true,"editDisposition":"rewind"})),
            )),
            Some("assistantText") => {
                let turn = row["turnId"].as_str().unwrap_or("");
                let mut actions = serde_json::Map::new();
                if Some(index) == retryable {
                    actions.insert("canRetry".into(), true.into());
                }
                if latest_by_turn.get(turn) == Some(&index)
                    && row["state"] == "complete"
                    && bound.contains(&index)
                    && headers
                        .get(turn)
                        .is_some_and(|state| state == "completedSuccess")
                {
                    actions.insert("canFork".into(), true.into());
                }
                wanted.push((
                    index,
                    (!actions.is_empty()).then_some(Value::Object(actions)),
                ));
            }
            _ => {}
        }
    }
    for (index, actions) in wanted {
        match actions {
            Some(actions) => session.rows[index]["actions"] = actions,
            None => {
                if let Some(row) = session.rows[index].as_object_mut() {
                    row.remove("actions");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(
            "s".into(),
            "w".into(),
            "p".into(),
            "m".into(),
            "none".into(),
            "e".into(),
            0,
        )
    }

    fn header(s: &mut Session, turn: &str, state: &str) -> usize {
        let mut row = s.row("turnHeader", turn, turn, 1);
        row["state"] = state.into();
        s.rows.push(row);
        s.rows.len() - 1
    }

    fn user(s: &mut Session, turn: &str, entity: &str, origin: &str) -> usize {
        let mut row = s.row("userInput", turn, entity, 1);
        row["origin"] = origin.into();
        s.rows.push(row);
        s.rows.len() - 1
    }

    fn assistant(s: &mut Session, turn: &str, entity: &str, state: &str) -> usize {
        let mut row = s.row("assistantText", turn, entity, 1);
        row["state"] = state.into();
        s.rows.push(row);
        s.rows.len() - 1
    }

    fn seqs(s: &Session) -> Vec<u64> {
        s.rows
            .iter()
            .map(|row| row["createdAtSeq"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn turn_started_rows_share_a_sequence_and_other_rows_advance() {
        let mut s = session();
        let h = header(&mut s, "t1", "completedSuccess");
        let u = user(&mut s, "t1", "u1", "realUser");
        let a = assistant(&mut s, "t1", "a1", "complete");
        imported_sequences(&mut s);
        assert_eq!(seqs(&s)[h], seqs(&s)[u], "TurnStarted creates both rows");
        assert!(seqs(&s)[a] > seqs(&s)[h], "assistant row is a later event");
    }

    #[test]
    fn model_change_marker_before_a_header_joins_that_turn_group() {
        let mut s = session();
        let mut marker = s.row("timelineMarker", "t1", "m1", 1);
        marker["marker"] = json!({"type":"modelChange"});
        s.rows.push(marker);
        let h = header(&mut s, "t2", "completedSuccess");
        imported_sequences(&mut s);
        assert_eq!(
            seqs(&s)[0],
            seqs(&s)[h],
            "Node emits the marker on TurnStarted"
        );
        let mut s = session();
        let mut marker = s.row("timelineMarker", "t1", "m1", 1);
        marker["marker"] = json!({"type":"compact"});
        s.rows.push(marker);
        imported_sequences(&mut s);
        assert_eq!(seqs(&s)[0], 1, "other timeline rows are independent events");
    }

    fn actions(s: &Session, index: usize) -> Value {
        s.rows[index].get("actions").cloned().unwrap_or(Value::Null)
    }

    #[test]
    fn every_turns_latest_assistant_forks_and_only_the_global_tail_retries() {
        let mut s = session();
        let h1 = header(&mut s, "t1", "completedSuccess");
        let u1 = user(&mut s, "t1", "u1", "realUser");
        let a1 = assistant(&mut s, "t1", "a1", "complete");
        let h2 = header(&mut s, "t2", "completedSuccess");
        let u2 = user(&mut s, "t2", "u2", "realUser");
        let a2 = assistant(&mut s, "t2", "a2", "complete");
        let bound = [h1, u1, a1, h2, u2, a2];
        imported_actions(&mut s, &bound);
        assert_eq!(actions(&s, a1), json!({"canFork":true}));
        assert_eq!(actions(&s, a2), json!({"canRetry":true,"canFork":true}));
        assert_eq!(
            actions(&s, u2),
            json!({"canEdit":true,"editDisposition":"rewind"})
        );
        assert_eq!(actions(&s, u1), Value::Null);
        assert_eq!(actions(&s, h2), Value::Null);
    }

    #[test]
    fn interrupted_turn_forfeits_fork_and_non_tail_assistant_only_forks_when_latest() {
        let mut s = session();
        let h1 = header(&mut s, "t1", "completedSuccess");
        let u1 = user(&mut s, "t1", "u1", "realUser");
        let a1 = assistant(&mut s, "t1", "a1", "complete");
        let a2 = assistant(&mut s, "t1", "a2", "complete");
        let h2 = header(&mut s, "t2", "completedInterrupted");
        let u2 = user(&mut s, "t2", "u2", "realUser");
        let a3 = assistant(&mut s, "t2", "a3", "complete");
        let bound = [h1, u1, a1, a2, h2, u2, a3];
        imported_actions(&mut s, &bound);
        assert_eq!(actions(&s, a1), Value::Null);
        assert_eq!(actions(&s, a2), json!({"canFork":true}));
        assert_eq!(actions(&s, a3), json!({"canRetry":true}));
    }

    #[test]
    fn unbound_rows_and_synthetic_causes_never_receive_actions() {
        let mut s = session();
        let h = header(&mut s, "t1", "completedSuccess");
        let synthetic = user(&mut s, "t1", "u1", "synthetic");
        let a1 = assistant(&mut s, "t1", "a1", "complete");
        imported_actions(&mut s, &[h, synthetic, a1]);
        assert_eq!(actions(&s, synthetic), Value::Null);
        assert_eq!(
            actions(&s, a1),
            json!({"canFork":true}),
            "no realUser cause means no retry"
        );
        let mut s = session();
        let h = header(&mut s, "t1", "completedSuccess");
        let u = user(&mut s, "t1", "u1", "realUser");
        let a = assistant(&mut s, "t1", "a1", "complete");
        imported_actions(&mut s, &[h]);
        assert_eq!(actions(&s, a), Value::Null, "unbound row is not a target");
        assert_eq!(
            actions(&s, u),
            Value::Null,
            "unbound latest input is not editable"
        );
    }
}
