//! 投影的行与 source 状态（TS `windowHostControllerProjection.ts` 的 `buildSourceRows` /
//! `workspaceFact` / `addressFor` 口径）。从 `projection.rs` 拆出以守住单文件行数上限。

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::overlays;

/// TS 投影的 `taskKey(address)`（本地 source）：`local\0<identity‖path>\0<taskId>`。
pub(crate) fn row_key(
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
) -> String {
    format!(
        "local\0{}\0{task_id}",
        workspace_identity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(workspace_path)
    )
}

/// 本地 source scope（TS `{kind:"local", workspacePath, workspaceIdentity?}`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalScope {
    pub(crate) workspace_path: String,
    pub(crate) workspace_identity: Option<String>,
}

impl LocalScope {
    pub(crate) fn new(workspace_path: &str, workspace_identity: Option<&str>) -> Self {
        Self {
            workspace_path: workspace_path.to_owned(),
            workspace_identity: workspace_identity
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        }
    }

    /// 身份键：`workspaceIdentity?.trim() || workspacePath`（AGENTS.md）。
    pub(super) fn workspace_key(&self) -> &str {
        self.workspace_identity
            .as_deref()
            .unwrap_or(&self.workspace_path)
    }

    /// TS `sourceKey({kind:"local", …})`。
    pub(crate) fn source_key(&self) -> String {
        format!("local\0{}", self.workspace_key())
    }

    /// TS 投影的 `taskKey(address)`（本地 source）。
    pub(super) fn row_key(&self, task_id: &str) -> String {
        row_key(
            &self.workspace_path,
            self.workspace_identity.as_deref(),
            task_id,
        )
    }

    /// TS `addressFor(scope, taskId)`：本地 source 不下发 `remoteSessionId`。
    pub(super) fn address(&self, task_id: &str) -> Value {
        let mut address = json!({
            "workspacePath": self.workspace_path,
            "taskId": task_id,
        });
        if let Some(identity) = &self.workspace_identity {
            address["workspaceIdentity"] = Value::from(identity.clone());
        }
        address
    }
}

/// 一个本地 source 的投影状态（TS `ControllerSource` 的本地子集）。
pub(super) struct Source {
    pub(super) scope: LocalScope,
    /// `sourceAvailability`：登记时 `offline`，首个成功快照转 `online`。
    pub(super) online: bool,
    /// `connectionState`：登记时 `connecting`，首个成功快照转 `online`。
    pub(super) connection_state: &'static str,
    /// 行键 → `{meta, membership, searchSnippets?}`（TS `memberships`）。
    pub(super) memberships: BTreeMap<String, Value>,
    /// taskId → overlay（TS `overlays`）。
    pub(super) overlays: BTreeMap<String, Value>,
    /// 行键 → 投影行（TS `rows`）。
    pub(super) rows: BTreeMap<String, Value>,
    /// 至少成功快照过一次（TS `sourceSnapshotTaskServices` 口径：非强制刷新据此跳过读库）。
    pub(super) refreshed: bool,
}

/// TS `buildSourceRows`：membership × overlay → 行（标题覆盖规则与 `statusFromActivity` 逐条照抄）。
pub(super) fn build_rows(source: &Source) -> BTreeMap<String, Value> {
    let mut rows = BTreeMap::new();
    for (key, membership) in &source.memberships {
        let Some(task_id) = membership["meta"]["taskId"].as_str() else {
            continue;
        };
        let overlay = source.overlays.get(task_id);
        let meta = merged_meta(&membership["meta"], overlay);
        let mut row = json!({
            "address": source.scope.address(task_id),
            "meta": meta,
            "membership": membership["membership"],
            "sourceAvailability": if source.online { "online" } else { "offline" },
            "liveStatus": overlay
                .map(|overlay| overlay["liveStatus"].clone())
                .unwrap_or_else(|| Value::from(overlays::default_live_status(&membership["meta"]))),
        });
        if let Some(activity) = overlay.and_then(|overlay| overlay.get("activity")) {
            row["activity"] = activity.clone();
        }
        if let Some(snippets) = membership.get("searchSnippets") {
            row["searchSnippets"] = snippets.clone();
        }
        rows.insert(key.clone(), row);
    }
    rows
}

/// TS `buildSourceRows` 的 meta 合并（`undefined` 字段整键省略，见子 spec §2 叠加表）。
fn merged_meta(meta: &Value, overlay: Option<&Value>) -> Value {
    let Some(overlay) = overlay else {
        return meta.clone();
    };
    let use_overlay_title = overlay["title"].is_string()
        && (meta["titleOverridden"] != Value::Bool(true) || overlay["titleSource"] == "custom");
    let mut merged = meta.clone();
    if let Some(record) = merged.as_object_mut() {
        if use_overlay_title {
            record.insert("title".to_owned(), overlay["title"].clone());
        }
        if overlay["titleSource"] == "custom" {
            record.insert("titleOverridden".to_owned(), Value::Bool(true));
        }
        if let Some(updated_at) = overlay.get("updatedAt").filter(|value| !value.is_null()) {
            record.insert("updatedAt".to_owned(), updated_at.clone());
        }
        if let Some(activity) = overlay.get("activity") {
            match overlays::status_from_activity(activity) {
                Some(status) => {
                    record.insert("status".to_owned(), Value::from(status));
                }
                None => {
                    record.remove("status");
                }
            }
        }
        // TS 直接赋值 `pendingInteraction: overlay.pendingInteraction`：overlay 缺该键时结果也
        // 不再携带 meta 里的旧值（`undefined` 在 JSON 里整键省略）。
        match overlay.get("pendingInteraction") {
            Some(pending) => {
                record.insert("pendingInteraction".to_owned(), pending.clone());
            }
            None => {
                record.remove("pendingInteraction");
            }
        }
    }
    merged
}

/// TS `workspaceFact(source)`（本地 source 不含 `remoteSessionId`）。
pub(super) fn workspace_fact(source: &Source) -> Value {
    let mut fact = json!({
        "workspacePath": source.scope.workspace_path,
        "sourceAvailability": if source.online { "online" } else { "offline" },
        "connectionState": source.connection_state,
    });
    if let Some(identity) = &source.scope.workspace_identity {
        fact["workspaceIdentity"] = Value::from(identity.clone());
    }
    fact
}

/// TS `Date.now()`。
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// TS `matchesTaskListMembershipKind`（`packages/shared/src/escode-protocol-v4/controller.ts`）。
pub(crate) fn matches_membership_kind(membership: &Value, kind: &str) -> bool {
    let pinned = membership["pinned"] == Value::Bool(true);
    let archived = membership["archived"] == Value::Bool(true);
    match kind {
        "pinned" => pinned && !archived,
        "archived" => archived,
        "timeline" => !pinned && !archived,
        "active" => !archived,
        _ => false,
    }
}
