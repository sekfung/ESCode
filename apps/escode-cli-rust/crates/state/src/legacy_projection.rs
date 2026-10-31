use crate::domain::session::Session;
use anyhow::{Result, ensure};
use rusqlite::Connection;
use serde_json::{Value, json};
pub(super) fn ledger(snapshot: &Connection, id: &str, session: &mut Session) -> Result<()> {
    let mut ledger=snapshot.prepare("SELECT id,payload,status,delivery,promoted_message_id FROM session_input WHERE session_id=?1 ORDER BY admitted_sequence")?;
    let inputs = ledger
        .query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (input, payload, status, delivery, promoted) in inputs {
        let payload: Value = serde_json::from_str(&payload)?;
        let command = payload["intent"]["sourceCommandId"]
            .as_str()
            .or_else(|| payload["conversationInputIntent"]["sourceCommandId"].as_str());
        if let Some(command) = command {
            let key = serde_json::to_string(&(Some(id), command))?;
            let accepted = status == "promoted";
            let mut ack = json!({"commandId":command,"status":if accepted{"accepted"}else{"failed"},"revisionAtDecision":0,"result":{"type":if accepted{"inputAccepted"}else{"inputDisposition"},"delivery":delivery}});
            if accepted {
                ack["result"]["inputId"] = promoted.unwrap_or(input).into();
            } else {
                ack["reasonCode"] = "fault.input.discardedOnRestart".into();
            }
            session.pending_acks.insert(key, ack);
        }
    }
    Ok(())
}
pub(super) fn validate_parts(parts: &[(String, Value)]) -> Result<()> {
    for (_, p) in parts {
        ensure!(
            matches!(
                p["type"].as_str(),
                Some(
                    "text"
                        | "reasoning"
                        | "file"
                        | "tool"
                        | "compaction"
                        | "timeline"
                        | "retry"
                        | "step-start"
                        | "step-finish"
                        | "snapshot"
                        | "patch"
                        | "agent"
                        | "subtask"
                )
            ),
            "Unsupported TS history part; preserve source and use TS for this workspace"
        );
    }
    Ok(())
}
pub(super) fn timeline(session: &mut Session, parts: &[(String, Value)], turn: &str, now: u64) {
    for (id, p) in parts {
        let marker = match p["type"].as_str() {
            Some("retry") => Some(
                json!({"type":"retryNotice","attempt":p["attempt"],"reasonCode":"provider.retry"}),
            ),
            Some("timeline") => match p["timelineType"].as_str() {
                Some("model_change") => model_change(session, p),
                Some("context_compaction") => Some(
                    json!({"type":"compact","origin":if p["trigger"]=="manual"{"manual"}else{"auto"},"status":if p["status"]=="completed"{"success"}else{"cancelled"}}),
                ),
                Some("session_fork") => Some(
                    json!({"type":"forkNotice","parentSessionId":p["parentSessionId"],"parentRowId":0}),
                ),
                Some("goal_verification") => Some(
                    json!({"type":"goalVerify","iteration":p["goalIteration"].as_u64().unwrap_or(0),"outcome":if p["verification"]["passed"]==true{"pass"}else{"notSatisfied"},"detail":p["verification"]["reason"].as_str().unwrap_or("")}),
                ),
                _ => None,
            },
            _ => None,
        };
        if let Some(marker) = marker {
            let mut row = session.row("timelineMarker", turn, id, now);
            row["lane"] = if marker["type"] == "modelChange" {
                "lightBoundary"
            } else {
                "turnTailBoundary"
            }
            .into();
            row["marker"] = marker;
            if let Some(command) = p["sourceCommandId"].as_str() {
                row["sourceCommandId"] = command.into();
            }
            session.rows.push(row);
        }
    }
}

/// Node 投影：没有来源的显式边界（∅→X，sourceLess）总会落 marker；有来源时，首轮之前
/// 为 silentInitial 不落，之后只在模型身份确实改变时落（思考深度变化不算）。
/// 修复：TS 自 migration 0020 起真实选择在 *Selection 字段，toModel 只是给旧 Reader 的
/// providerID/modelID 兼容对象；原先读 toModel.providerId 得到 null，标记不满足任何
/// modelChange 变体，App schema 拒绝整页 rows。对齐 TS decodeStoredPart：只读 *Selection。
fn model_change(session: &Session, p: &Value) -> Option<Value> {
    let show = match (
        model_selection(&p["fromModelSelection"]),
        model_selection(&p["toModelSelection"]),
    ) {
        (None, _) => true,
        (Some(from), Some(to)) => {
            session.rows.iter().any(|r| r["kind"] == "turnHeader")
                && (from.0, from.1) != (to.0, to.1)
        }
        (Some(_), None) => false,
    };
    if !show {
        return None;
    }
    let (provider, model, thought) = model_selection(&p["toModelSelection"])?;
    let mut marker =
        json!({"type":"modelChange","toProvider":provider,"toModel":model,"toThought":thought});
    if let Some((provider, model, _)) = model_selection(&p["fromModelSelection"]) {
        marker["fromProvider"] = provider.into();
        marker["fromModel"] = model.into();
    }
    Some(marker)
}

/// Node `modelChangeToModelOf`：最后一条 model_change timeline part 且带 `toModel` 时，
/// 整条 assistant 消息只承载模型切换记录，不进入轮次统计或行投影。
pub(super) fn model_change_timeline(parts: &[(String, Value)]) -> Option<(String, Value)> {
    parts
        .iter()
        .rev()
        .find(|(_, p)| p["type"] == "timeline" && p["timelineType"] == "model_change")
        .filter(|(_, p)| p["toModel"].is_object())
        .map(|(id, p)| (id.clone(), p.clone()))
}

/// Node 把模型切换标记推迟到下一个 TurnStarted：标记与 header/userInput 同属该轮。
pub(super) fn model_marker(session: &mut Session, turn: &str, id: &str, p: &Value, now: u64) {
    let Some(marker) = model_change(session, p) else {
        return;
    };
    let mut row = session.row("timelineMarker", turn, id, now);
    row["lane"] = "lightBoundary".into();
    row["marker"] = marker;
    if let Some(command) = p["sourceCommandId"].as_str() {
        row["sourceCommandId"] = command.into();
    }
    session.rows.push(row);
}

/// TS `decodeTimelineSelection`：providerId/modelId 必须是非空字符串，否则视为无选择。
fn model_selection(value: &Value) -> Option<(String, String, String)> {
    let provider = value["providerId"]
        .as_str()
        .filter(|v| !v.trim().is_empty())?;
    let model = value["modelId"].as_str().filter(|v| !v.trim().is_empty())?;
    let thought = value["options"]["reasoningLevel"].as_str().unwrap_or("");
    Some((provider.to_owned(), model.to_owned(), thought.to_owned()))
}

pub(super) fn reasoning(message: &mut Value, parts: &[(String, Value)]) {
    let mut thinking = vec![];
    let mut responses = vec![];
    for (_, p) in parts.iter().filter(|(_, p)| p["type"] == "reasoning") {
        let a = &p["metadata"]["anthropic"];
        if let Some(data) = a["redactedData"].as_str() {
            thinking.push(json!({"type":"redacted_thinking","data":data}));
        } else if let Some(signature) = a["signature"].as_str() {
            thinking.push(json!({"type":"thinking","thinking":p["text"],"signature":signature}));
        }
        let r = &p["metadata"]["openai"];
        if let (Some(id), Some(data)) = (
            r["itemId"].as_str(),
            r["reasoningEncryptedContent"].as_str(),
        ) {
            responses.push(json!({"type":"reasoning","id":id,"encrypted_content":data,"summary":[{"type":"summary_text","text":p["text"]}]}));
        }
    }
    if !thinking.is_empty() {
        message["_escode_anthropic_thinking"] = thinking.into();
    }
    if !responses.is_empty() {
        message["_escode_responses_reasoning"] = responses.into();
    }
}

/// 读取行时同时保留原始 JSON 文本：冷导入工具输入需要按 TS 写入的键序还原
/// `inputText`，而本工作区未启用 serde_json `preserve_order`（app-server 字节链路依赖该事实）。
pub(super) fn items_raw(
    conn: &Connection,
    sql: &str,
    id: &str,
) -> Result<Vec<(String, Value, String)>> {
    let mut q = conn.prepare(sql)?;
    q.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?
    .map(|r| {
        let (id, data) = r?;
        let value = serde_json::from_str(&data)?;
        Ok((id, value, data))
    })
    .collect()
}

pub(super) fn items(conn: &Connection, sql: &str, id: &str) -> Result<Vec<(String, Value)>> {
    Ok(items_raw(conn, sql, id)?
        .into_iter()
        .map(|(id, value, _)| (id, value))
        .collect())
}

/// Node transcript hydration 的 messageEndAtMs：completed/created 与 reasoning/tool 结束时间的最大值。
pub(super) fn message_end_at(message: &Value, parts: &[(String, Value)]) -> Option<u64> {
    let mut end = message["time"]["completed"]
        .as_u64()
        .or_else(|| message["time"]["created"].as_u64());
    for (_, part) in parts {
        let part_end = match part["type"].as_str() {
            Some("reasoning") => part["time"]["end"]
                .as_u64()
                .or_else(|| part["time"]["start"].as_u64()),
            Some("tool") => part["state"]["time"]["end"]
                .as_u64()
                .or_else(|| part["state"]["time"]["start"].as_u64()),
            _ => None,
        };
        if let Some(part_end) = part_end {
            end = Some(end.map_or(part_end, |value| value.max(part_end)));
        }
    }
    end
}

/// 冷导入 turnHeader 对齐 Node：endedAt/activeMs 取本轮消息跨度，historyRoundCount 取 assistant 消息数。
pub(super) fn finalize_turn(
    session: &mut Session,
    index: Option<usize>,
    started: u64,
    ended: u64,
    rounds: u64,
    open_tools: &mut Vec<usize>,
) {
    // Node TurnComplete(cancelled) 统一关闭仍处于 pending/running 的工具行；
    // endedAt 取轮次结束事件时间（整轮消息 end 的最大值），不是工具自身 start。
    for row in open_tools.drain(..) {
        session.rows[row]["endedAt"] = ended.into();
    }
    let Some(index) = index else { return };
    let row = &mut session.rows[index];
    row["endedAt"] = ended.into();
    if row["executionKind"] != "controlOnly" {
        row["activeMs"] = ended.saturating_sub(started).into();
        row["historyRoundCount"] = rounds.into();
    } else if rounds > 0 {
        row["historyRoundCount"] = rounds.into();
    }
}

/// 中断轮：会话整体标记完成但可恢复，且该轮 turnHeader 不再宣称成功（影响 fork 资格）。
pub(super) fn mark_interrupted(session: &mut Session, turn: &str) {
    session.phase = "completedInterrupted".into();
    session.auto_drain = false;
    for row in &mut session.rows {
        if row["kind"] == "turnHeader" && row["turnId"] == turn {
            row["state"] = "completedInterrupted".into();
        }
    }
}

/// 首轮之前的边界标记（如 ∅→X 模型标记）在 Node 中属于它之后的第一轮；空 turnId 会被 App schema 拒绝。
pub(super) fn attach_pending_rows(session: &mut Session, turn: &str) {
    for row in session.rows.iter_mut().filter(|r| r["turnId"] == "") {
        row["turnId"] = turn.into();
        row["productTurnId"] = turn.into();
    }
}

/// 冷导入的已完成 turnHeader；createdAt/startedAt/endedAt 都从用户消息时间起算，收口时再更新。
pub(super) fn imported_turn_header(session: &mut Session, turn: &str, now: u64) -> usize {
    let mut header = session.row("turnHeader", turn, turn, now);
    header["origin"] = "userInput".into();
    header["state"] = "completedSuccess".into();
    header["startedAt"] = now.into();
    header["endedAt"] = now.into();
    // Node 冷恢复为每轮合成 usage=0 的 ModelComplete，turnHeader 因此总是写 outputTokens。
    header["outputTokens"] = 0.into();
    session.rows.push(header);
    session.rows.len() - 1
}

/// 冷导入的 userInput 行；附件展示元信息从持久 file part 还原。
pub(super) fn imported_user_input(
    session: &mut Session,
    turn: &str,
    mid: &str,
    now: u64,
    message: &Value,
    parts: &[(String, Value)],
    text: &str,
) -> usize {
    let mut row = session.row("userInput", turn, mid, now);
    row["text"] = text.into();
    row["origin"] = if message["synthetic"] == true {
        "synthetic"
    } else {
        "realUser"
    }
    .into();
    if let Some(command) = message["anchor"]["sourceCommandId"].as_str() {
        row["sourceCommandId"] = command.into();
    }
    // Node `attachmentMetasOfMessage`：data: 内联附件没有稳定内容引用，行投影改用
    // `turn-attachment/<rowId>/<index>` 占位；此前直接把 data URL 写进 ref，冷/热行不一致。
    let attachments=parts.iter().filter(|(_,p)|p["type"]=="file").enumerate().map(|(index,(_,p))|{
        let url=p["url"].as_str().unwrap_or("");
        let stable=!url.is_empty()&&!url.starts_with("data:");
        let file_name=match p["filename"].as_str(){
            Some(name)=>name.to_owned(),
            None=>{
                let base=if stable{url.rsplit(['/','\\']).next().unwrap_or("")}else{""};
                if base.is_empty(){format!("attachment-{}",index+1)}else{base.to_owned()}
            }
        };
        let reference=if stable{url.to_owned()}else{format!("turn-attachment/{}/{}",row["rowId"].as_u64().unwrap_or(0),index)};
        json!({"ref":reference,"fileName":file_name,"mime":p["mime"].as_str().unwrap_or("application/octet-stream"),"bytes":p["metadata"]["sizeBytes"].as_u64().unwrap_or(0)})
    }).collect::<Vec<_>>();
    if !attachments.is_empty() {
        row["attachments"] = attachments.into();
    }
    session.rows.push(row);
    session.rows.len() - 1
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

    /// 真实 TS（0020 之后）写出的形状：toModel 为旧 Reader 兼容对象，真实值在 *Selection。
    fn stored(to: Value, from: Value) -> Value {
        json!({"type":"timeline","timelineType":"model_change","status":"completed",
            "toModel":{"providerID":"legacy","modelID":"legacy","variant":"high","label":"x"},
            "fromModel":{"providerID":"legacy","modelID":"legacy","label":"x"},
            "toModelSelection":to,"fromModelSelection":from})
    }

    fn after_a_turn() -> Session {
        let mut s = session();
        s.rows.push(json!({"kind":"turnHeader"}));
        s
    }

    #[test]
    fn model_change_reads_the_selection_fields_like_ts() {
        let mut s = after_a_turn();
        let part = stored(
            json!({"providerId":"zai","modelId":"glm-5","options":{"reasoningLevel":"high"},"label":"GLM"}),
            json!({"providerId":"zai","modelId":"glm-4","label":"GLM"}),
        );
        timeline(&mut s, &[("p1".into(), part)], "t", 1);
        assert_eq!(
            s.rows[1]["marker"],
            json!({"type":"modelChange","toProvider":"zai","toModel":"glm-5","toThought":"high","fromProvider":"zai","fromModel":"glm-4"})
        );
    }

    #[test]
    fn model_change_without_a_valid_selection_is_skipped_and_partial_source_is_empty_origin() {
        let mut s = after_a_turn();
        timeline(
            &mut s,
            &[("p1".into(), stored(Value::Null, Value::Null))],
            "t",
            1,
        );
        assert_eq!(
            s.rows.len(),
            1,
            "TS decode drops toModel, Node projection skips the marker"
        );
        let part = stored(
            json!({"providerId":"zai","modelId":"glm-5"}),
            json!({"providerId":"zai"}),
        );
        timeline(&mut s, &[("p2".into(), part)], "t", 1);
        let marker = &s.rows[1]["marker"];
        assert!(marker.get("fromProvider").is_none() && marker.get("fromModel").is_none());
        assert_eq!(marker["toThought"], "");
    }

    #[test]
    fn model_change_before_the_first_turn_or_to_the_same_model_has_no_marker() {
        let mut s = session();
        let change = stored(
            json!({"providerId":"zai","modelId":"glm-5"}),
            json!({"providerId":"zai","modelId":"glm-4"}),
        );
        timeline(&mut s, &[("p1".into(), change)], "t", 1);
        assert!(
            s.rows.is_empty(),
            "first turn with a known source is silentInitial in Node"
        );
        let source_less = stored(json!({"providerId":"zai","modelId":"glm-5"}), Value::Null);
        timeline(&mut s, &[("p0".into(), source_less)], "t", 1);
        assert_eq!(s.rows.len(), 1, "an explicit ∅→X boundary always renders");
        let mut s = after_a_turn();
        let same = stored(
            json!({"providerId":"zai","modelId":"glm-5","options":{"reasoningLevel":"high"}}),
            json!({"providerId":"zai","modelId":"glm-5"}),
        );
        timeline(&mut s, &[("p1".into(), same)], "t", 1);
        assert_eq!(
            s.rows.len(),
            1,
            "a thought-only change is not a model identity change"
        );
    }
}
