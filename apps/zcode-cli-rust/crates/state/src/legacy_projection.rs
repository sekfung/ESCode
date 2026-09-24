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
                // 修复：TS 自 migration 0020 起真实选择在 *Selection 字段，toModel 只是给旧 Reader 的
                // providerID/modelID 兼容对象；原先读 toModel.providerId 得到 null，标记不满足任何
                // modelChange 变体，App schema 拒绝整页 rows。对齐 TS decodeStoredPart：只读 *Selection。
                // Node 投影：没有来源的显式边界（∅→X，sourceLess）总会落 marker；有来源时，首轮之前
                // 为 silentInitial 不落，之后只在模型身份确实改变时落（思考深度变化不算）。
                Some("model_change")
                    if match (
                        model_selection(&p["fromModelSelection"]),
                        model_selection(&p["toModelSelection"]),
                    ) {
                        (None, _) => true,
                        (Some(from), Some(to)) => {
                            session.rows.iter().any(|r| r["kind"] == "turnHeader")
                                && (from.0, from.1) != (to.0, to.1)
                        }
                        (Some(_), None) => false,
                    } =>
                {
                    model_selection(&p["toModelSelection"]).map(|(provider, model, thought)| {
                    let mut m = json!({"type":"modelChange","toProvider":provider,"toModel":model,"toThought":thought});
                    if let Some((provider, model, _)) = model_selection(&p["fromModelSelection"]) {
                        m["fromProvider"] = provider.into();
                        m["fromModel"] = model.into();
                    }
                    m
                    })
                }
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
        message["_zcode_anthropic_thinking"] = thinking.into();
    }
    if !responses.is_empty() {
        message["_zcode_responses_reasoning"] = responses.into();
    }
}

pub(super) fn items(conn: &Connection, sql: &str, id: &str) -> Result<Vec<(String, Value)>> {
    let mut q = conn.prepare(sql)?;
    q.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?
    .map(|r| {
        let (id, data) = r?;
        Ok((id, serde_json::from_str(&data)?))
    })
    .collect()
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
