//! 快照 → 任务索引 meta 的投影（`snapshotToMeta`）与消息窗口/ETag 语义。
//!
//! 上游真相源：`packages/services/src/escode-agent/escodeTaskServiceAdapter.ts` 的
//! `snapshotToMeta` / `deriveTitleFromSnapshot` / `fromESCodeMode` / `fromESCodeGoal` /
//! `normalizeSnapshotMessageLimit` / `buildSnapshotHistory` / `limitTaskSnapshotMessages`，
//! 以及 `packages/services/src/escode-agent/escodeConfigOptions.ts` 的
//! `formatTaskMetaModelSelectionFromSnapshot`。
//!
//! `applyOverlayToMeta`（title/unreadAt 的进程内 overlay）**不在本函数内**：overlay 属 Host
//! 内存态，由调用方在写入索引前后叠加（Rust Host 目前没有 overlay，等价于空 overlay）。

use serde_json::{Map, Value};

use super::config_options::format_model_picker_value;
use super::status;
use super::visible;

/// TS `snapshotToMeta`。
pub fn snapshot_to_meta(snapshot: &Value) -> Value {
    let session = field(snapshot, "session").unwrap_or(&Value::Null);
    let projection = field(snapshot, "projection").unwrap_or(&Value::Null);
    let settings = field(snapshot, "settings").unwrap_or(&Value::Null);
    let workspace = field(session, "workspace").unwrap_or(&Value::Null);

    let session_id = text_field(session, "sessionId").unwrap_or_default();
    // TS `snapshot.session.traceId ?? generateTraceId(session.sessionId)`（随机 UUID）。
    let trace_id =
        text_field(session, "traceId").unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut meta = Map::new();
    meta.insert("taskId".to_owned(), Value::from(session_id));
    meta.insert("traceId".to_owned(), Value::from(trace_id));
    meta.insert(
        "title".to_owned(),
        Value::from(visible::visible_session_title(
            field(session, "title").and_then(Value::as_str),
            field(snapshot, "messages")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice),
            field(projection, "target"),
            None,
        )),
    );
    insert_text(
        &mut meta,
        "workspacePath",
        text_field(workspace, "workspacePath"),
    );
    insert_text(
        &mut meta,
        "workspaceIdentity",
        text_field(workspace, "workspaceIdentity"),
    );
    insert_number(&mut meta, "createdAt", number_field(session, "createdAt"));
    insert_number(&mut meta, "updatedAt", number_field(session, "updatedAt"));
    if let Some(mode) = text_field(session, "mode") {
        meta.insert("mode".to_owned(), Value::from(from_escode_mode(&mode)));
    }
    meta.insert(
        "model".to_owned(),
        Value::from(format_task_meta_model_selection_from_snapshot(snapshot)),
    );
    insert_text(
        &mut meta,
        "thoughtLevel",
        text_field(
            field(settings, "thoughtLevel").unwrap_or(&Value::Null),
            "current",
        ),
    );
    // TS `GLM_PROVIDER`：任务索引只服务当前 glm provider 的行。
    meta.insert("provider".to_owned(), Value::from(super::GLM_PROVIDER));
    if let Some(task_status) = status::task_status_from_snapshot(snapshot) {
        meta.insert("status".to_owned(), Value::from(task_status));
    }
    if let Some(last_error) = field(projection, "lastError").filter(|value| truthy(value)) {
        meta.insert("lastError".to_owned(), last_error_projection(last_error));
    }
    if let Some(target) = field(projection, "target").filter(|value| truthy(value)) {
        meta.insert("target".to_owned(), from_escode_goal(target));
    }
    Value::Object(meta)
}

/// TS `snapshotToMeta` 的 `lastError` 投影：`code` 回退 `type`，`detail`/`attribution` 条件键。
fn last_error_projection(error: &Value) -> Value {
    let mut projected = Map::new();
    // TS `snapshot.projection.lastError.code ?? snapshot.projection.lastError.type`。
    projected.insert(
        "code".to_owned(),
        text_field(error, "code")
            .or_else(|| text_field(error, "type"))
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    // TS 是真值判定（空串不下发）。
    insert_text(
        &mut projected,
        "detail",
        text_field(error, "detail").filter(|value| !value.is_empty()),
    );
    // service snapshot 是 mobile replayable/cold task meta 的来源，不能让它与 UI 直接投影的
    // lastError 产生归因漂移。`attribution` 是**对象**（`errorAttributionSchema`），必须原样透传：
    // 把它当字符串处理会让 meta_json 落成非法行、索引读回退化成列投影。
    if let Some(attribution) = field(error, "attribution").filter(|value| truthy(value)) {
        projected.insert("attribution".to_owned(), attribution.clone());
    }
    insert_text(&mut projected, "message", text_field(error, "message"));
    Value::Object(projected)
}

/// TS `fromESCodeMode`：只有 `build` 之外的模式原样保留（`build` → `build`）。
pub fn from_escode_mode(mode: &str) -> String {
    mode.to_owned()
}

/// TS `fromESCodeGoal`：未知 status 归 `active`，缺省计数归 0，时间取 `time.*` 再回退扁平字段。
pub fn from_escode_goal(goal: &Value) -> Value {
    let time = field(goal, "time").unwrap_or(&Value::Null);
    let status = text_field(goal, "status");
    let status = match status.as_deref() {
        Some("active" | "paused" | "budget_limited" | "complete") => {
            status.unwrap_or_else(|| "active".to_owned())
        }
        _ => "active".to_owned(),
    };
    // JS 数字经 `JSON.stringify` 落成整数形态（`tokensUsed: 0` 不是 `0.0`）。
    serde_json::json!({
        "sessionID": text_field(goal, "sessionID").or_else(|| text_field(goal, "sessionId")).unwrap_or_default(),
        "targetID": text_field(goal, "targetID").or_else(|| text_field(goal, "targetId")).unwrap_or_default(),
        "objective": text_field(goal, "objective").unwrap_or_default(),
        "summaryTitle": text_field(goal, "summaryTitle").map(Value::from).unwrap_or(Value::Null),
        "status": status,
        "tokenBudget": field(goal, "tokenBudget").filter(|value| value.is_number()).cloned().unwrap_or(Value::Null),
        "tokensUsed": number_value(number_field(goal, "tokensUsed").unwrap_or(0.0)),
        "timeUsedSeconds": number_value(number_field(goal, "timeUsedSeconds").unwrap_or(0.0)),
        "time": {
            "created": number_value(number_field(time, "created").or_else(|| number_field(goal, "createdAt")).unwrap_or(0.0)),
            "updated": number_value(number_field(time, "updated").or_else(|| number_field(goal, "updatedAt")).unwrap_or(0.0)),
        },
    })
}

/// TS `formatTaskMetaModelSelectionFromSnapshot`：最近一条消息的模型优先于 `settings.current`。
pub fn format_task_meta_model_selection_from_snapshot(snapshot: &Value) -> String {
    let messages = field(snapshot, "messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let latest = messages
        .iter()
        .rev()
        .find_map(|message| field(message, "info").and_then(|info| field(info, "model")))
        .cloned();
    let current = field(snapshot, "settings")
        .and_then(|settings| field(settings, "model"))
        .and_then(|model| field(model, "current"))
        .cloned();
    match latest {
        Some(model) => format_model_picker_value(Some(&model)),
        None => format_model_picker_value(current.as_ref()),
    }
}

/// TS `normalizeSnapshotMessageLimit`：非正数/非有限值/缺省一律视为无限制。
pub fn normalize_message_limit(message_limit: Option<f64>) -> Option<u64> {
    let limit = message_limit.filter(|limit| limit.is_finite() && *limit > 0.0)?;
    Some(limit.floor() as u64)
}

/// TS `buildSnapshotHistory`：`{truncatedBefore, totalMessages}`。
pub fn build_snapshot_history(total_messages: u64, message_limit: u64) -> Value {
    serde_json::json!({
        "truncatedBefore": total_messages > message_limit,
        "totalMessages": total_messages,
    })
}

/// TS `limitTaskSnapshotMessages`：只有真的截断时才写 `history`（否则整个键不出现）。
pub fn limit_task_snapshot_messages(mut snapshot: Value, message_limit: Option<f64>) -> Value {
    let Some(limit) = normalize_message_limit(message_limit) else {
        return snapshot;
    };
    let Some(messages) = field(&snapshot, "messages").and_then(Value::as_array) else {
        return snapshot;
    };
    let total = messages.len() as u64;
    let history = build_snapshot_history(total, limit);
    let truncated = total > limit;
    let tail = if truncated {
        messages[messages.len() - limit as usize..].to_vec()
    } else {
        messages.clone()
    };
    if let Some(record) = snapshot.as_object_mut() {
        record.insert("messages".to_owned(), Value::Array(tail));
        // 手机 replayable 首屏为了性能只拿尾部窗口；UI 不能再用「返回条数是否等于 limit」
        // 猜测更早历史，所以截断标记必须随快照下发。
        record.insert("history".to_owned(), history);
    }
    snapshot
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn insert_text(record: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        record.insert(key.to_owned(), Value::from(value));
    }
}

fn insert_number(record: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        record.insert(key.to_owned(), number_value(value));
    }
}

/// JS 数字写进 JSON 的口径：整数值不写小数点（`JSON.stringify(42)` 是 `42`）。
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    field(value, key).and_then(Value::as_f64)
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    field(value, key)
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot_fixture() -> Value {
        json!({
            "session": {
                "sessionId": "task_1", "traceId": "trace_1", "title": "",
                "createdAt": 100, "updatedAt": 900, "mode": "plan", "status": "completed",
                "workspace": {"workspaceKey": "/w", "workspacePath": "/w"},
            },
            "settings": {
                "model": {"current": {"providerId": "glm", "modelId": "glm-4"}},
                "thoughtLevel": {"current": "high"},
            },
            "projection": {
                "status": "completed", "pendingPermissions": [], "activeToolCalls": [],
                "lastError": {"code": "runtime_error", "detail": "d", "message": "boom",
                              "attribution": "runtime"},
                "target": {"targetId": "t1", "sessionId": "task_1", "objective": "把迁移做完",
                           "status": "paused", "tokensUsed": 12, "timeUsedSeconds": 3,
                           "time": {"created": 50, "updated": 60}},
            },
            "runtime": {},
            "messages": [
                {"info": {"role": "user", "messageId": "u1", "time": {"created": 100}},
                 "parts": [{"type": "text", "text": "第一条用户输入"}]},
                {"info": {"role": "assistant", "messageId": "a1", "time": {"created": 120, "completed": 200},
                          "model": {"providerId": "custom", "modelId": "kimi", "options": {"reasoningLevel": "high"}},
                          "semantics": {"origin": "real_user", "kind": "assistant_response",
                                        "uiVisibility": "visible", "transcriptVisibility": "visible"}},
                 "parts": [{"type": "text", "text": "回复"}]},
            ],
        })
    }

    #[test]
    fn meta_projection_covers_goal_error_and_model_selection() {
        let meta = snapshot_to_meta(&snapshot_fixture());
        assert_eq!(meta["taskId"], json!("task_1"));
        assert_eq!(meta["traceId"], json!("trace_1"));
        // title 为空 → 取首条真实用户输入。
        assert_eq!(meta["title"], json!("第一条用户输入"));
        assert_eq!(meta["workspacePath"], json!("/w"));
        assert_eq!(meta["createdAt"], json!(100));
        assert_eq!(meta["updatedAt"], json!(900));
        assert_eq!(meta["mode"], json!("plan"));
        // task meta 的模型取最近消息实际使用的模型（不是 settings.current）。
        assert_eq!(meta["model"], json!("custom/kimi$high"));
        assert_eq!(meta["thoughtLevel"], json!("high"));
        assert_eq!(meta["provider"], json!("glm"));
        // lastError 存在 → status=error（压过 session.status=completed）。
        assert_eq!(meta["status"], json!("error"));
        assert_eq!(
            meta["lastError"],
            json!({"code": "runtime_error", "detail": "d", "attribution": "runtime", "message": "boom"})
        );
        assert_eq!(meta["target"]["targetID"], json!("t1"));
        assert_eq!(meta["target"]["status"], json!("paused"));
        assert_eq!(meta["target"]["objective"], json!("把迁移做完"));
        assert_eq!(
            meta["target"]["time"],
            json!({"created": 50, "updated": 60})
        );
        // workspaceIdentity 缺失 → 键不存在（不回 null）。
        assert!(meta.get("workspaceIdentity").is_none());
    }

    #[test]
    fn meta_omits_last_error_target_and_status_when_absent() {
        let meta = snapshot_to_meta(&json!({
            "session": {"sessionId": "t", "title": "标题", "status": "idle",
                        "workspace": {"workspacePath": "/w"}, "mode": "build"},
            "settings": {"model": {}, "thoughtLevel": {}},
            "projection": {},
            "runtime": {},
            "messages": [],
        }));
        assert!(meta.get("lastError").is_none());
        assert!(meta.get("target").is_none());
        // idle 不是任务状态（TS statusFromESCodeSession 只认 running/waiting/paused/error/completed）。
        assert!(meta.get("status").is_none());
        assert!(meta.get("thoughtLevel").is_none());
        assert_eq!(meta["title"], json!("标题"));
        assert_eq!(meta["model"], json!(""));
        assert_eq!(meta["workspaceIdentity"], Value::Null);
    }

    #[test]
    fn goal_status_falls_back_to_active_and_time_falls_back_to_flat_fields() {
        let goal = from_escode_goal(&json!({
            "sessionId": "s", "targetId": "t", "status": "weird",
            "createdAt": 7, "updatedAt": 8, "summaryTitle": null, "tokenBudget": "x",
        }));
        assert_eq!(goal["sessionID"], json!("s"));
        assert_eq!(goal["targetID"], json!("t"));
        assert_eq!(goal["status"], json!("active"));
        assert_eq!(goal["summaryTitle"], Value::Null);
        assert_eq!(goal["tokenBudget"], Value::Null);
        assert_eq!(goal["tokensUsed"], json!(0));
        assert_eq!(goal["time"], json!({"created": 7, "updated": 8}));
    }

    #[test]
    fn message_limit_windows_the_tail_and_marks_truncation() {
        let snapshot =
            json!({"messages": [{"id": 1}, {"id": 2}, {"id": 3}], "meta": {"taskId": "t"}});
        // 无限制：原样返回，不写 history。
        assert_eq!(
            limit_task_snapshot_messages(snapshot.clone(), None),
            snapshot
        );
        assert_eq!(
            limit_task_snapshot_messages(snapshot.clone(), Some(0.0)),
            snapshot
        );
        assert_eq!(
            limit_task_snapshot_messages(snapshot.clone(), Some(-1.0)),
            snapshot
        );
        // 不截断：写 history 但消息不动。
        let kept = limit_task_snapshot_messages(snapshot.clone(), Some(3.0));
        assert_eq!(kept["messages"].as_array().expect("messages").len(), 3);
        assert_eq!(
            kept["history"],
            json!({"truncatedBefore": false, "totalMessages": 3})
        );
        // 截断：只留尾部窗口。
        let truncated = limit_task_snapshot_messages(snapshot, Some(2.0));
        assert_eq!(truncated["messages"], json!([{"id": 2}, {"id": 3}]));
        assert_eq!(
            truncated["history"],
            json!({"truncatedBefore": true, "totalMessages": 3})
        );
        // 小数 limit 向下取整。
        assert_eq!(normalize_message_limit(Some(2.9)), Some(2));
        assert_eq!(normalize_message_limit(Some(f64::NAN)), None);
    }
}
