//! `query*` 共用的小工具：成员行 identity 解析、purpose map 的键借用、分组颜色表、
//! meta_json 非法时的极薄条目。集中一处避免三份拷贝漂移。

use serde_json::{Value, json};

/// TS 的 7 色（`isTaskGroupColor`）；非法值回 gray。
pub(super) const GROUP_COLORS: [&str; 7] =
    ["gray", "red", "orange", "yellow", "green", "blue", "purple"];

/// 成员行的 identity 解析（与 TS `resolveTaskIndexRowWorkspaceIdentity` 同规则）。
pub(super) fn resolve_member_identity(
    workspace_key: &str,
    workspace_path: &str,
    column_identity: Option<&str>,
) -> Option<String> {
    if workspace_key == workspace_path && column_identity == Some(workspace_path) {
        return None;
    }
    if column_identity == Some(workspace_key) {
        return Some(workspace_key.to_owned());
    }
    if workspace_key.starts_with("remote:") && !workspace_key.contains('/') {
        return Some(workspace_key.to_owned());
    }
    None
}

/// purpose map 需要借用 key：条目循环内 key 生命周期覆盖整个循环，这里转 'static 泄漏可接受
/// （数量 = scope 数，个位数）。避免为 HashMap<&str, &str> 再造一层所有权。
pub(super) fn leak_key(key: &str) -> &'static str {
    Box::leak(key.to_owned().into_boxed_str())
}

/// TS 的 7 色（`isTaskGroupColor`）；非法值回 gray。（TS `rowToTaskListItem` 在 meta 非法时同样回退列投影）。
pub(super) fn fallback_item(row: &Value) -> Value {
    json!({
        "taskId": row["task_id"],
        "title": row["title"],
        "workspacePath": row["workspace_path"],
        "createdAt": row["created_at"],
        "updatedAt": row["updated_at"],
        "mode": row["mode"],
    })
}
