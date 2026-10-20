//! 快照读面的服务编排：请求整形（含 replayable 的索引 hint 回填）、投影 + 窗口 + 索引回写、ETag。
//!
//! 上游真相源：`packages/services/src/escode-agent/escodeTaskServiceAdapter.ts` 的
//! `getTaskSnapshot`（2876 起）、`getTaskSnapshotWithEtag`（2954 起）、`getTaskConfigOptions`（3014）
//! 与 `resumeSnapshot`（1175）/ `resolveTaskIndexResumeHints`（1462）。
//!
//! 顺序是可见行为，不能重排：hint 读取在 resume 之前、索引写在使用**未截窗**的投影之后。

use serde_json::{Map, Value};

use super::meta::limit_task_snapshot_messages;
use super::{ProjectOptions, project_snapshot, settings_to_config_options, snapshot_to_meta};
use crate::tasks_index::{SyncOptions, TasksIndex};

/// TS `normalizeSnapshotMessageLimit` + `getTaskSnapshot`：`messageLimit` 是可选入参。
pub fn message_limit_of(params: &Value) -> Option<f64> {
    params.get("messageLimit").and_then(Value::as_f64)
}

/// TS `params.clientMode === "web-remote-replayable"`。
pub fn is_replayable(params: &Value) -> bool {
    text(params, "clientMode").as_deref() == Some("web-remote-replayable")
}

/// TS `snapshotToESCode(snapshot, { includeEmptyPendingElicitations: clientMode === "web-remote-replayable" })`。
pub fn project_options_of(params: &Value) -> ProjectOptions {
    ProjectOptions {
        include_empty_pending_elicitation: is_replayable(params),
    }
}

/// TS `resumeSnapshot` 的服务级参数（`options.escodeAgentService.resumeSession(...)` 的入参）。
///
/// 形状与 TS 逐键一致（`workspacePath`/`workspaceIdentity`/`sessionId` + 可选 `thoughtLevel`/
/// `mcpServers`/`toolDenylist`），运行时链路的 `workspace` 引用由 `forward::runtime_target` 补齐。
/// `model` 不在其中：TS 侧它只用于 agent 层的连接/陈旧模型守卫，`buildSessionResumeParams`
/// （`packages/services/src/escode-agent/escodeAgentService.ts:761`）**不下发**它。
pub fn resume_snapshot_request(
    task_id: &str,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    thought_level: Option<&str>,
    mcp_servers: Option<&Value>,
    tool_denylist: Option<&Value>,
) -> Value {
    let mut request = Map::new();
    request.insert("workspacePath".to_owned(), Value::from(workspace_path));
    if let Some(identity) = workspace_identity.filter(|value| !value.trim().is_empty()) {
        request.insert("workspaceIdentity".to_owned(), Value::from(identity));
    }
    request.insert("sessionId".to_owned(), Value::from(task_id));
    if let Some(thought_level) = thought_level.filter(|value| !value.is_empty()) {
        request.insert("thoughtLevel".to_owned(), Value::from(thought_level));
    }
    if let Some(mcp_servers) = mcp_servers.filter(|value| !value.is_null()) {
        request.insert("mcpServers".to_owned(), mcp_servers.clone());
    }
    // 工具面约束必须和 create 路径一致随 resume 下发，否则冷恢复重建 runtime 后会丢失 allow/deny 隔离。
    if let Some(tool_denylist) = tool_denylist.filter(|value| !value.is_null()) {
        request.insert("toolDenylist".to_owned(), tool_denylist.clone());
    }
    Value::Object(request)
}

/// TS `getTaskSnapshot` 的 resume 参数整形：replayable 首屏用索引里的历史模型/档位补 hint。
///
/// 返回 `(model, thought_level)`：`model` 只用于日志与后续 agent 层语义（不下发运行时），
/// `thought_level` 会随 resume 下发。
pub fn resume_hints(
    index: Option<&TasksIndex>,
    params: &Value,
    workspace_path: &str,
    workspace_identity: Option<&str>,
) -> Result<(Option<String>, Option<String>), crate::tasks_index::TasksIndexError> {
    let explicit_model = text(params, "model").filter(|value| !value.is_empty());
    let explicit_thought = text(params, "thoughtLevel").filter(|value| !value.is_empty());
    // TS `shouldBackfillTaskIndexResumeHints`：只有远程 workspace replayable 首屏、且未被 UI 显式解析时才回填。
    let should_backfill = is_replayable(params)
        && text(params, "resumeModelPolicy").as_deref() != Some("ui-resolved-only")
        && (explicit_model.is_none() || explicit_thought.is_none());
    if !should_backfill {
        return Ok((explicit_model, explicit_thought));
    }
    let Some(index) = index else {
        return Ok((explicit_model, explicit_thought));
    };
    let task_id = text(params, "taskId").unwrap_or_default();
    if task_id.is_empty() {
        return Ok((explicit_model, explicit_thought));
    }
    // TS `taskIndexRepo.getTaskMeta(params).catch(...)`：读失败按「无 hint」继续，不阻断恢复。
    let meta = index
        .get_task_meta(workspace_path, workspace_identity, &task_id)?
        .unwrap_or(Value::Null);
    let hint_model = text(&meta, "model")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let hint_thought = text(&meta, "thoughtLevel")
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Ok((
        explicit_model.or(hint_model),
        explicit_thought.or(hint_thought),
    ))
}

/// TS `getTaskSnapshot` 的会话分支：`snapshotToESCode` → `limitTaskSnapshotMessages` →
/// `syncTaskMeta`（索引行即返回值里的 `meta`）。
pub fn read_session_snapshot(
    index: &TasksIndex,
    snapshot: &Value,
    message_limit: Option<f64>,
    options: ProjectOptions,
) -> Result<Value, crate::tasks_index::TasksIndexError> {
    let mut projected =
        limit_task_snapshot_messages(project_snapshot(snapshot, options), message_limit);
    // `syncTaskMeta` 继续保留用户手动标题（title_overridden 走既有合并规则）。
    // `limitTaskSnapshotMessages` 只改 messages/history，meta 仍来自未截窗的投影。
    let meta = snapshot_to_meta(snapshot);
    let indexed = match meta.as_object() {
        Some(meta) => index.sync_task_meta(meta, &SyncOptions::default())?,
        None => meta,
    };
    if let Some(record) = projected.as_object_mut() {
        record.insert("meta".to_owned(), indexed);
    }
    Ok(projected)
}

/// TS `getTaskSnapshotWithEtag`：`sha256(JSON.stringify(snapshot))`。
///
/// 登记差异：TS 的 `JSON.stringify` 保留对象**插入序**，Rust 的 `serde_json`（未开
/// `preserve_order`）按键排序，因此同一快照的 etag 与 TS **不逐字节相同**；客户端只用它做
/// 条件请求，同一 Host 内自洽。若要逐字节一致需做键序对齐（见子 spec §6.4.4）。
pub fn snapshot_etag(snapshot: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(snapshot).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

/// TS `getTaskSnapshotWithEtag` 的 `ifNoneMatch` 命中形状。
pub fn not_modified(etag: &str) -> Value {
    serde_json::json!({"snapshot": Value::Null, "etag": etag, "notModified": true})
}

/// TS `getTaskConfigOptions`：`resumeSnapshot` 后只投影 `settings`。
pub fn config_options_from_snapshot(snapshot: &Value) -> Value {
    let settings = snapshot.get("settings").cloned().unwrap_or(Value::Null);
    Value::Array(settings_to_config_options(&settings))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .as_object()
        .and_then(|record| record.get(key))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resume_request_keeps_only_the_fields_typescript_sends() {
        let bare = resume_snapshot_request("t1", "/w", None, None, None, None);
        let mut keys = bare
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, vec!["sessionId", "workspacePath"]);
        assert!(bare.get("model").is_none(), "model 只用于 agent 层守卫");

        let full = resume_snapshot_request(
            "t2",
            "/w",
            Some("remote:ssh:host"),
            Some("high"),
            Some(&json!([{"name": "mcp1"}])),
            Some(&json!(["Bash"])),
        );
        assert_eq!(full["workspacePath"], json!("/w"));
        assert_eq!(full["workspaceIdentity"], json!("remote:ssh:host"));
        assert_eq!(full["thoughtLevel"], json!("high"));
        assert_eq!(full["mcpServers"], json!([{"name": "mcp1"}]));
        assert_eq!(full["toolDenylist"], json!(["Bash"]));
        // 空白身份视同未提供（AGENTS.md Workspace Identity）。
        let blank = resume_snapshot_request("t3", "/w", Some("  "), None, None, None);
        assert!(blank.get("workspaceIdentity").is_none());
    }

    #[test]
    fn project_options_follow_client_mode() {
        assert!(
            !project_options_of(&json!({"clientMode": "desktop-continuous"}))
                .include_empty_pending_elicitation
        );
        assert!(
            project_options_of(&json!({"clientMode": "web-remote-replayable"}))
                .include_empty_pending_elicitation
        );
    }

    #[test]
    fn etag_is_sha256_hex_and_not_modified_is_conditional() {
        let etag = snapshot_etag(&json!({"a": 1}));
        assert_eq!(etag.len(), 64);
        assert!(etag.chars().all(|character| character.is_ascii_hexdigit()));
        // 同一输入稳定；不同输入不同。
        assert_eq!(etag, snapshot_etag(&json!({"a": 1})));
        assert_ne!(etag, snapshot_etag(&json!({"a": 2})));
        assert_eq!(
            not_modified(&etag),
            json!({"snapshot": null, "etag": etag, "notModified": true})
        );
    }
}
