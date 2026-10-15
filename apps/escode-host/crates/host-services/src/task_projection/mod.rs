//! 任务域服务层投影（agent 协议快照 → `ESCodeTaskSnapshot`）。
//!
//! 上游真相源是 TS 服务层：`packages/services/src/escode-agent/escodeTaskServiceAdapter.ts`
//! 的 `snapshotToESCode` 与它调用的一串共享投影 helper。Rust Host 只拿到运行时的
//! **agent 协议快照**（`session/resume` / `session/read` 同形），任务索引行与 UI 快照都由
//! Host 投影（与 TS 同结构）。
//!
//! 模块划分（每个文件对齐一段 TS 语义）：
//! - `policy` / `visible` / `status`：可见性策略、可见消息与 goal 迭代号、任务状态派生；
//! - `messages`（+ `message_parts` / `message_merge`）：`mapMessage` 与相邻 assistant 合并；
//! - `timeline`：session fork 与 goal verifier divider 的补插与重排；
//! - `background`：后台任务通知与控制项；
//! - `permissions`：待处理权限 → `permission_request` / `elicitation_request`；
//! - `config_options`：会话设置 → `configOptions`；
//! - `meta`：快照 → 任务索引 meta、消息窗口与 `history`。
//!
//! 登记差异见 `docs/specs/rust-u2-tasks-index.md` §6.4.4（grayscale 工具面 flag、legacy 分支、
//! live tool projection 合并、ETag 键序）。

mod background;
mod background_controls;
mod config_options;
mod index_sync;
mod message_merge;
mod message_parts;
mod message_timeline;
mod messages;
mod meta;
mod permissions;
mod policy;
mod service;
mod status;
mod timeline;
mod visible;

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod fixture_tests;

pub use config_options::{format_model_picker_value, settings_to_config_options};
pub use index_sync::{
    IndexOverrides, index_meta_from_snapshot, searchable_text_from_snapshot, sync_snapshot_to_index,
};
pub use messages::{coalesce_consecutive_assistants, map_message};
pub use meta::{
    build_snapshot_history, from_escode_goal, from_escode_mode, limit_task_snapshot_messages,
    normalize_message_limit, snapshot_to_meta,
};
pub use permissions::is_user_input_backed_permission_tool_name;
pub use service::{
    config_options_from_snapshot, message_limit_of, not_modified, project_options_of,
    read_session_snapshot, resume_hints, resume_snapshot_request, snapshot_etag,
};
pub use status::task_status_from_snapshot;
pub use timeline::{
    normalize_goal_verification_timeline_order, snapshot_goal_active_iteration_count,
};

use serde_json::{Map, Value};

/// TS `GLM_PROVIDER`（`ESCODE_AGENT_PROVIDER`）：任务索引只服务当前 provider 的行。
pub(crate) const GLM_PROVIDER: &str = "glm";

/// TS `snapshotToESCode` 的服务级选项。
#[derive(Clone, Copy, Default)]
pub struct ProjectOptions {
    /// `clientMode === "web-remote-replayable"`：空 `pendingElicitations` 也要表达「队列已清空」。
    pub include_empty_pending_elicitation: bool,
}

/// TS `snapshotToESCode`：agent 协议快照 → `ESCodeTaskSnapshot`（不含索引回写）。
pub fn project_snapshot(snapshot: &Value, options: ProjectOptions) -> Value {
    let messages = field(snapshot, "messages")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let target = field(snapshot, "projection").and_then(|projection| field(projection, "target"));
    let active_iteration = timeline::snapshot_goal_active_iteration_count(snapshot);
    let iterations = visible::goal_iteration_by_assistant_message_id(
        messages,
        (active_iteration > 0).then_some(active_iteration),
        target,
    );
    let notifications = background::collect_notifications_by_tool_use_id(messages);
    let mapped = visible::user_visible_messages(messages)
        .iter()
        .map(|message| {
            let role = field(message, "info").and_then(|info| field(info, "role"));
            let iteration = (role.and_then(Value::as_str) == Some("assistant"))
                .then(|| {
                    field(message, "info")
                        .and_then(|info| field(info, "messageId"))
                        .and_then(Value::as_str)
                        .and_then(|id| iterations.get(id).copied())
                })
                .flatten();
            map_message(message, iteration, Some(&notifications))
        })
        .collect::<Vec<Value>>();

    let mut messages = coalesce_consecutive_assistants(mapped);
    messages = timeline::add_session_fork_fallback(messages, snapshot);
    messages = timeline::add_goal_verification_timeline_fallback(messages, snapshot);
    messages = timeline::normalize_goal_verification_timeline_order(messages);

    let session_id = field(snapshot, "session")
        .and_then(|session| field(session, "sessionId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let pending_permissions = field(snapshot, "projection")
        .and_then(|projection| field(projection, "pendingPermissions"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let permission_events = pending_permissions
        .iter()
        .filter(|permission| {
            !field(permission, "toolName")
                .and_then(Value::as_str)
                .is_some_and(permissions::is_user_input_backed_permission_tool_name)
        })
        .map(|permission| permissions::pending_permission_to_stream_event(&session_id, permission))
        .collect::<Vec<Value>>();
    let elicitation_events = pending_permissions
        .iter()
        .filter(|permission| {
            field(permission, "toolName")
                .and_then(Value::as_str)
                .is_some_and(permissions::is_user_input_backed_permission_tool_name)
        })
        .filter_map(|permission| {
            permissions::pending_user_input_backed_permission_to_elicitation_event(
                &session_id,
                permission,
            )
        })
        .collect::<Vec<Value>>();
    let background_controls = background_controls::parse_control_items(
        field(snapshot, "projection")
            .and_then(|projection| field(projection, "backgroundJobs"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice),
    );

    let settings = field(snapshot, "settings").unwrap_or(&Value::Null);
    let runtime = field(snapshot, "runtime").unwrap_or(&Value::Null);
    let mut projected_runtime = Map::new();
    insert_from(
        &mut projected_runtime,
        "activeTurnKind",
        field(runtime, "activeTurnKind"),
    );
    projected_runtime.insert("apiRetry".to_owned(), Value::Null);
    if let Some(usage) = runtime_context_usage(runtime)
        .or_else(|| projection_context_usage(field(snapshot, "projection").unwrap_or(&Value::Null)))
    {
        projected_runtime.insert("contextUsage".to_owned(), usage);
    }
    projected_runtime.insert(
        "pendingPermissions".to_owned(),
        Value::Array(permission_events),
    );
    projected_runtime.insert(
        "backgroundBashJobs".to_owned(),
        Value::Array(background_controls),
    );
    if options.include_empty_pending_elicitation || !elicitation_events.is_empty() {
        projected_runtime.insert(
            "pendingElicitations".to_owned(),
            Value::Array(elicitation_events),
        );
    }
    projected_runtime.insert("pendingCommands".to_owned(), Value::Array(Vec::new()));
    projected_runtime.insert("plan".to_owned(), todos_to_plan_steps(snapshot));
    projected_runtime.insert("goalStats".to_owned(), goal_stats(snapshot));
    projected_runtime.insert(
        "goalVerifications".to_owned(),
        field(runtime, "goalVerifications")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(Value::Null),
    );
    projected_runtime.insert(
        "goalVerificationTimeline".to_owned(),
        field(runtime, "goalVerificationTimeline")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(Value::Null),
    );
    projected_runtime.insert("todoGroups".to_owned(), todo_groups(snapshot));

    let mut result = Map::new();
    result.insert("meta".to_owned(), snapshot_to_meta(snapshot));
    result.insert("messages".to_owned(), Value::Array(messages));
    // 文件变更 diff 由 CLI 侧的 fileChanges 流维护，task facade 固定空数组（TS 同）。
    result.insert("fileChanges".to_owned(), Value::Array(Vec::new()));
    result.insert(
        "configOptions".to_owned(),
        Value::Array(settings_to_config_options(settings)),
    );
    // agent 协议 snapshot 含 `/compact` 等命令；task facade 必须保留，否则 replayable/legacy
    // task restore 会把 UI 的 slashCommands 回填成空。
    result.insert(
        "slashCommands".to_owned(),
        field(snapshot, "slashCommands")
            .and_then(Value::as_array)
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    result.insert("runtime".to_owned(), Value::Object(projected_runtime));
    Value::Object(result)
}

fn runtime_context_usage(runtime: &Value) -> Option<Value> {
    let usage = field(runtime, "contextUsage")?;
    let size = number_field(usage, "size")?;
    let used = number_field(usage, "used")?;
    if size <= 0.0 || used <= 0.0 {
        return None;
    }
    // session resume 时 protocol projection 可能还没重放主轮次 usage，
    // runtime.contextUsage 来自持久化 assistant token 记录，应优先用于恢复旧 task UI。
    let mut projected = Map::new();
    projected.insert("used".to_owned(), number_value(used));
    projected.insert("size".to_owned(), number_value(size));
    projected.insert(
        "cost".to_owned(),
        field(usage, "cost").cloned().unwrap_or(Value::Null),
    );
    insert_from(&mut projected, "cache", field(usage, "cache"));
    insert_from(&mut projected, "breakdown", field(usage, "breakdown"));
    Some(Value::Object(projected))
}

fn projection_context_usage(projection: &Value) -> Option<Value> {
    let window = number_field(projection, "contextWindow")?;
    let used = number_field(projection, "contextUsed")?;
    if window <= 0.0 || used <= 0.0 {
        return None;
    }
    Some(serde_json::json!({"used": used, "size": window, "cost": Value::Null}))
}

/// TS `sessionTodosToPlanSteps`：空列表回 `null`（不是空数组）。
fn todos_to_plan_steps(snapshot: &Value) -> Value {
    let todos = field(snapshot, "todos")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if todos.is_empty() {
        return Value::Null;
    }
    Value::Array(
        todos
            .iter()
            .enumerate()
            .map(|(index, todo)| {
                serde_json::json!({
                    "id": format!("todo-{index}"),
                    "status": field(todo, "status").cloned().unwrap_or(Value::Null),
                    "title": field(todo, "content").cloned().unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

/// TS `sessionGoalStatsToRuntime`：`{...stats}` 或 `null`。
fn goal_stats(snapshot: &Value) -> Value {
    field(snapshot, "goalStats")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or(Value::Null)
}

/// TS `sessionTodoGroupsToRuntime`：空列表回 `null`；组内 todo 的 id 是 `{groupId}-todo-{i}`。
fn todo_groups(snapshot: &Value) -> Value {
    let groups = field(snapshot, "todoGroups")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if groups.is_empty() {
        return Value::Null;
    }
    Value::Array(
        groups
            .iter()
            .map(|group| {
                let group_id = text_field(group, "id").unwrap_or_default();
                let todos = field(group, "todos")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .enumerate()
                    .map(|(index, todo)| {
                        serde_json::json!({
                            "id": format!("{group_id}-todo-{index}"),
                            "status": field(todo, "status").cloned().unwrap_or(Value::Null),
                            "title": field(todo, "content").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect::<Vec<Value>>();
                let mut projected = Map::new();
                projected.insert("id".to_owned(), Value::from(group_id));
                insert_from(&mut projected, "source", field(group, "source"));
                // TS 用真值判定：0/空串的 goalIteration 等字段不下发。
                insert_truthy(
                    &mut projected,
                    "goalIteration",
                    field(group, "goalIteration"),
                );
                insert_truthy(&mut projected, "targetId", field(group, "targetId"));
                insert_truthy(&mut projected, "startedAt", field(group, "startedAt"));
                insert_truthy(&mut projected, "updatedAt", field(group, "updatedAt"));
                projected.insert("todos".to_owned(), Value::Array(todos));
                Value::Object(projected)
            })
            .collect(),
    )
}

fn insert_truthy(record: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| truthy(value)) {
        record.insert(key.to_owned(), value.clone());
    }
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

fn insert_from(record: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| !value.is_null()) {
        record.insert(key.to_owned(), value.clone());
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
    field(value, key).and_then(Value::as_str).map(str::to_owned)
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}
