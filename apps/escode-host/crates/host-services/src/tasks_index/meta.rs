//! `meta_json` → `ESCodeTaskMeta` 投影（TS `taskIndexRepo.ts` 的 `rowToMeta` 同序同语义）。
//!
//! TS 先按 `escodeTaskMetaSchema`（zod）校验 `meta_json`：
//! - 通过 → 以解析结果为主（未声明的键被 zod 剥掉），再用标量列覆盖
//!   `taskId`/`workspacePath`/`workspaceIdentity`/`unreadAt`，并按 `meta_json ?? 列` 兜底
//!   `cronAutomationId`；
//! - 不通过（JSON 解析失败、必填缺失、类型/枚举非法）→ 回退到纯列投影。
//!
//! Rust 侧实现「必填字段 + 已声明可选字段」的最小校验与白名单投影（见
//! `docs/specs/rust-u2-tasks-index.md` §3 的登记差异：不复制 zod 的深层对象校验）。
//! 差异只在**损坏行**上可见：TS 走列回退，Rust 走 meta 投影。

use serde_json::{Map, Value, json};

/// 任务模式枚举（TS `escodeTaskModeSchema`）。
const MODES: [&str; 11] = [
    "default",
    "yolo",
    "guarded",
    "plan",
    "edit",
    "acceptEdits",
    "auto",
    "dontAsk",
    "bypassPermissions",
    "autoEdit",
    "build",
];
/// 持久化状态枚举（TS `escodeTaskPersistStatusSchema`）。
const STATUSES: [&str; 3] = ["running", "completed", "error"];
/// 迁移来源枚举（TS `escodeTaskMigrationSourceSchema`）。
const MIGRATION_SOURCES: [&str; 1] = ["claudeCode"];
/// 工作区用途枚举（TS `workspacePurpose`）。
const WORKSPACE_PURPOSES: [&str; 2] = ["project", "conversation"];
/// 历史 provider 值（TS `legacyESCodeProviderSchema`）：全部归一为 `glm`。
const LEGACY_PROVIDERS: [&str; 5] = ["claude", "opencode", "gemini", "codex", "glm"];

/// 行列名常量（与 `TASK_ROW_SELECT` 索引一致，集中在此避免两处硬编码漂移）。
pub(super) struct Row<'a> {
    value: &'a Value,
}

impl<'a> Row<'a> {
    pub(super) fn new(value: &'a Value) -> Self {
        Self { value }
    }
    fn text(&self, key: &str) -> Option<String> {
        self.value[key].as_str().map(str::to_owned)
    }
    fn number(&self, key: &str) -> Option<i64> {
        self.value[key].as_i64()
    }
    fn required_text(&self, key: &str) -> String {
        self.text(key).unwrap_or_default()
    }
}

/// `rowToMeta`：`meta_json` 合法则用 meta 投影，否则用列回退。
pub(super) fn to_task_meta(row: &Value) -> Option<Value> {
    let row = Row::new(row);
    let workspace_identity = resolve_workspace_identity(&row);
    let parsed = serde_json::from_str::<Value>(&row.required_text("meta_json"))
        .ok()
        .filter(is_valid_meta);
    if let Some(meta) = parsed {
        let mut projected = meta.as_object().cloned().unwrap_or_default();
        // 标量与主键投影必须与行一致（旧 meta_json 里的身份可能属于旧远端）。
        projected.insert("taskId".into(), json!(row.required_text("task_id")));
        projected.insert(
            "workspacePath".into(),
            json!(row.required_text("workspace_path")),
        );
        match &workspace_identity {
            Some(identity) => projected.insert("workspaceIdentity".into(), json!(identity)),
            None => projected.remove("workspaceIdentity"),
        };
        projected.insert(
            "unreadAt".into(),
            row.number("unread_at")
                .map_or(Value::Null, |value| json!(value)),
        );
        if projected["unreadAt"].is_null() {
            projected.remove("unreadAt");
        }
        projected.insert(
            "cronAutomationId".into(),
            optional_string(
                projected.get("cronAutomationId"),
                row.text("cron_automation_id"),
            ),
        );
        projected.insert(
            "titleOverridden".into(),
            json!(row.number("title_overridden") == Some(1)),
        );
        prune_nulls(&mut projected);
        return Some(Value::Object(projected));
    }
    Some(column_fallback_meta(&row, workspace_identity))
}

/// TS `resolveTaskIndexRowWorkspaceIdentity`：只有与 `workspace_key` 一致的独立身份才返回。
fn resolve_workspace_identity(row: &Row<'_>) -> Option<String> {
    let workspace_key = row.required_text("workspace_key");
    let workspace_path = row.required_text("workspace_path");
    let column_identity = row.text("workspace_identity");
    if workspace_key == workspace_path
        && column_identity.as_deref() == Some(workspace_path.as_str())
    {
        return None;
    }
    let trimmed = column_identity.as_deref().map(str::trim);
    if trimmed == Some(workspace_key.as_str()) {
        return Some(workspace_key);
    }
    if is_remote_workspace_identity(&workspace_key) {
        return Some(workspace_key);
    }
    None
}

/// TS `isRemoteWorkspaceIdentity`：远端身份形如 `remote:<kind>:<id>`（非路径）。
fn is_remote_workspace_identity(value: &str) -> bool {
    value.starts_with("remote:") && !value.contains(std::path::MAIN_SEPARATOR)
}

/// `meta_json` 合法性：必填字段齐全 + 已声明字段类型/枚举合法（zod 的浅层等价）。
fn is_valid_meta(meta: &Value) -> bool {
    let Some(object) = meta.as_object() else {
        return false;
    };
    let non_empty = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    };
    let non_negative = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_i64)
            .is_some_and(|value| value >= 0)
    };
    if !non_empty("taskId")
        || !non_empty("traceId")
        || !object.get("title").is_some_and(Value::is_string)
        || !non_empty("workspacePath")
        || !non_negative("createdAt")
        || !non_negative("updatedAt")
    {
        return false;
    }
    if !object
        .get("mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| MODES.contains(&mode))
    {
        return false;
    }
    let optional_non_empty = [
        "workspaceIdentity",
        "thoughtLevel",
        "forkedFromTaskId",
        "cronAutomationId",
    ];
    if optional_non_empty.iter().any(|key| {
        object
            .get(*key)
            .is_some_and(|value| value.as_str().is_none_or(|text| text.trim().is_empty()))
    }) {
        return false;
    }
    let optional_number = ["runtimeEpoch", "unreadAt"];
    if optional_number.iter().any(|key| {
        object
            .get(*key)
            .is_some_and(|value| value.as_i64().is_none_or(|n| n < 0))
    }) {
        return false;
    }
    if object
        .get("titleOverridden")
        .is_some_and(|value| !value.is_boolean())
        || object.get("model").is_some_and(|value| !value.is_string())
        || object.get("provider").is_some_and(|value| {
            !value
                .as_str()
                .is_some_and(|p| LEGACY_PROVIDERS.contains(&p))
        })
        || object.get("migrationSource").is_some_and(|value| {
            !value
                .as_str()
                .is_some_and(|source| MIGRATION_SOURCES.contains(&source))
        })
        || object.get("status").is_some_and(|value| {
            !value
                .as_str()
                .is_some_and(|status| STATUSES.contains(&status))
        })
        || object.get("workspacePurpose").is_some_and(|value| {
            !value
                .as_str()
                .is_some_and(|purpose| WORKSPACE_PURPOSES.contains(&purpose))
        })
    {
        return false;
    }
    true
}

/// 列回退投影：TS 走这条路径时字段较薄（没有 `meta_json` 独有字段）。
pub(super) fn fallback_meta(row: &Value) -> Value {
    column_fallback_meta(&Row::new(row), resolve_workspace_identity(&Row::new(row)))
}

fn column_fallback_meta(row: &Row<'_>, workspace_identity: Option<String>) -> Value {
    let mut meta = Map::new();
    let task_id = row.required_text("task_id");
    meta.insert("taskId".into(), json!(task_id));
    meta.insert("traceId".into(), json!(format!("escode-{task_id}")));
    meta.insert("title".into(), json!(row.required_text("title")));
    meta.insert(
        "titleOverridden".into(),
        json!(row.number("title_overridden") == Some(1)),
    );
    meta.insert(
        "workspacePath".into(),
        json!(row.required_text("workspace_path")),
    );
    if let Some(identity) = workspace_identity {
        meta.insert("workspaceIdentity".into(), json!(identity));
    }
    meta.insert(
        "createdAt".into(),
        json!(row.number("created_at").unwrap_or(0)),
    );
    meta.insert(
        "updatedAt".into(),
        json!(row.number("updated_at").unwrap_or(0)),
    );
    meta.insert("mode".into(), json!(row.required_text("mode")));
    for (key, value) in [
        ("model", row.text("model")),
        ("migrationSource", row.text("migration_source")),
        ("forkedFromTaskId", row.text("forked_from_task_id")),
        ("cronAutomationId", row.text("cron_automation_id")),
    ] {
        if let Some(value) = value {
            meta.insert(key.into(), json!(value));
        }
    }
    if let Some(provider) = row.text("provider") {
        // 旧行 provider 列可能残留三方 CLI 值；回退投影统一归一为 glm。
        meta.insert("provider".into(), json!("glm"));
        let _ = provider;
    }
    if let Some(unread) = row.number("unread_at") {
        meta.insert("unreadAt".into(), json!(unread));
    }
    if let Some(status) = row.text("task_status") {
        meta.insert("status".into(), json!(status));
    }
    Value::Object(meta)
}

/// `meta_json ?? 列` 兜底：两侧都空则去掉该键（TS 是 `undefined`）。
fn optional_string(from_meta: Option<&Value>, from_column: Option<String>) -> Value {
    match from_meta.and_then(Value::as_str) {
        Some(value) if !value.is_empty() => json!(value),
        _ => from_column.map_or(Value::Null, |value| json!(value)),
    }
}

/// `undefined` 语义：投影里值为 `null` 的键直接移除（JS 对象不会有 null 键）。
fn prune_nulls(object: &mut Map<String, Value>) {
    object.retain(|_, value| !value.is_null());
}

#[cfg(test)]
#[path = "meta_tests.rs"]
mod tests;
