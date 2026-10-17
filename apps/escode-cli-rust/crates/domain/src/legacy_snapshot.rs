//! Compatibility projection for the Host task index; Session remains the only owner.
use super::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn messages(s: &Session, cwd: &str) -> Vec<Value> {
    let mut result: Vec<Value> = vec![];
    let mut positions = BTreeMap::new();
    let mut parent = None;
    let mut topics = s.legacy_topic_records.iter().peekable();
    for (row_index, row) in s.rows.iter().enumerate() {
        // 旧 bot_topic_context 记录按导入时的行位置还原（docs/specs/rust-app-p0.md）。
        while let Some(record) = topics.next_if(|r| {
            r["rowAnchor"]
                .as_u64()
                .is_some_and(|a| a as usize <= row_index)
        }) {
            result.push(topic_message(s, record));
        }
        let kind = row["kind"].as_str().unwrap_or("");
        if !matches!(
            kind,
            "userInput" | "assistantText" | "reasoning" | "toolCall"
        ) {
            continue;
        }
        let user = kind == "userInput";
        let id = if user {
            &row["entityId"]
        } else {
            row.get("assistantResponseId").unwrap_or(&row["entityId"])
        };
        let Some(id) = id.as_str() else { continue };
        if user {
            parent = Some(id.to_owned());
        }
        let key = (user, id.to_owned());
        let index = *positions.entry(key).or_insert_with(|| {
            let mut info = json!({"messageId":id,"sessionId":s.id,"role":if user{"user"}else{"assistant"},"time":{"created":row["createdAt"]},"agent":"main"});
            if !user {
                info["parentMessageId"] = parent.as_deref().unwrap_or(id).into();
                info["path"] = json!({"cwd":cwd,"root":cwd});
                info["cost"] = 0.into();
                info["tokens"] = json!({"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}});
            } else if row["origin"] != "realUser" {
                info["synthetic"] = true.into();
                info["visibility"] = "model-only".into();
            }
            result.push(json!({"info":info,"parts":[]}));
            result.len()-1
        });
        let mut part =
            json!({"partId":format!("row-{}",row["rowId"]),"messageId":id,"sessionId":s.id});
        if kind == "toolCall" {
            part["type"] = "tool".into();
            part["callId"] = row["toolCallId"].clone();
            part["tool"] = row["toolName"].clone();
            let input = serde_json::from_str::<Value>(row["inputText"].as_str().unwrap_or("{}"))
                .ok()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            let mut state = json!({"input":input,"startedAt":row.get("startedAt").unwrap_or(&row["createdAt"])});
            if matches!(row["status"].as_str(), Some("running" | "pendingApproval")) {
                state["status"] = "running".into();
            } else {
                state["completedAt"] = row.get("endedAt").unwrap_or(&row["createdAt"]).clone();
                state["metadata"] = json!({});
                if row["status"] == "success" {
                    state["status"] = "completed".into();
                    state["title"] = row["toolName"].clone();
                    state["output"] = row["output"]["text"].as_str().unwrap_or("").into();
                } else {
                    state["status"] = "error".into();
                    state["error"] = row["error"]["message"]
                        .as_str()
                        .unwrap_or("Tool execution interrupted")
                        .into();
                }
            }
            part["state"] = state;
        } else {
            part["type"] = if kind == "reasoning" {
                "reasoning"
            } else {
                "text"
            }
            .into();
            part["text"] = row["text"].as_str().unwrap_or("").into();
        }
        result[index]["parts"].as_array_mut().unwrap().push(part);
        if user && let Some(attachments) = row["attachments"].as_array() {
            for (i, a) in attachments.iter().enumerate() {
                let mut file = json!({"partId":format!("row-{}-file-{i}",row["rowId"]),"messageId":id,"sessionId":s.id,"type":"file","mime":a["mime"],"url":a["ref"]});
                if let Some(name) = a.get("fileName") {
                    file["filename"] = name.clone();
                }
                result[index]["parts"].as_array_mut().unwrap().push(file);
            }
        }
    }
    result.extend(topics.map(|record| topic_message(s, record)));
    result
}

/// 旧话题背景的 legacy 消息形状（TS 存储原样：synthetic model-only user + source/metadata）。
fn topic_message(s: &Session, record: &Value) -> Value {
    let id = record["messageId"].as_str().unwrap_or_default();
    let mut metadata = json!({});
    for key in ["checkpoint", "displayText"] {
        if let Some(value) = record.get(key) {
            metadata[key] = value.clone();
        }
    }
    json!({
        "info": {"messageId":id,"sessionId":s.id,"role":"user","time":{"created":record["createdAt"]},
            "agent":"main","synthetic":true,"visibility":"model-only","source":"bot_topic_context",
            "metadata":metadata},
        "parts": [{"partId":format!("{id}-text"),"messageId":id,"sessionId":s.id,"type":"text",
            "text":record["text"]}],
    })
}
