//! deferred 草稿注册表（TS `createESCodeDeferredDraftRegistry`，
//! `packages/services/src/escode-session/escodeSessionDraftRegistry.ts`）。
//!
//! `persistence: "deferred"` 的会话只用于让 toolbar 与 runtime 共用同一份状态：这类空会话
//! **不能**进 tasks-index（否则侧边栏/搜索会出现没有用户输入的会话），且此后该会话的状态变更
//! 也必须被挡在索引之外，直到首次 `sendPrompt`（`promoteDeferredDraftSession` 显式提升）。
//!
//! 键与 TS 同：`workspaceIdentity?.trim() || workspacePath` + `\0` + `sessionId`
//! （AGENTS.md Workspace Identity）。

use std::collections::BTreeSet;
use std::sync::Mutex;

use serde_json::Value;

/// 进程内草稿注册表（Host 唯一实例，跨 channel 共享）。
#[derive(Default)]
pub struct DeferredDrafts {
    keys: Mutex<BTreeSet<String>>,
}

impl DeferredDrafts {
    pub fn new() -> Self {
        Self::default()
    }

    /// TS `remember`：键取**快照里**的 workspace（workspaceIdentity 回退请求参数）。
    pub fn remember(&self, params: &Value, snapshot: &Value) {
        let session = snapshot.get("session").cloned().unwrap_or(Value::Null);
        let workspace = session.get("workspace").cloned().unwrap_or(Value::Null);
        let workspace_path = workspace
            .get("workspacePath")
            .and_then(Value::as_str)
            .or_else(|| params.get("workspacePath").and_then(Value::as_str))
            .unwrap_or_default();
        let workspace_identity = workspace
            .get("workspaceIdentity")
            .and_then(Value::as_str)
            .or_else(|| params.get("workspaceIdentity").and_then(Value::as_str));
        let session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.keys()
            .insert(session_key(workspace_path, workspace_identity, session_id));
    }

    pub fn has(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        session_id: &str,
    ) -> bool {
        self.keys()
            .contains(&session_key(workspace_path, workspace_identity, session_id))
    }

    /// TS `forget`（`closeSession` / `closeDeferredDraftSession` / `promoteDeferredDraftSession`）。
    pub fn forget(&self, workspace_path: &str, workspace_identity: Option<&str>, session_id: &str) {
        self.keys()
            .remove(&session_key(workspace_path, workspace_identity, session_id));
    }

    /// 进程内草稿数量（诊断/测试）。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.keys().len()
    }

    fn keys(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn session_key(workspace_path: &str, workspace_identity: Option<&str>, session_id: &str) -> String {
    let identity = workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(workspace_path);
    format!("{identity}\u{0}{session_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn draft_keys_follow_workspace_identity_and_snapshot_workspace() {
        let drafts = DeferredDrafts::new();
        // 快照带 workspaceIdentity 时以快照为准（TS 同）。
        drafts.remember(
            &json!({"workspacePath": "/w", "workspaceIdentity": "/w"}),
            &json!({"session": {"sessionId": "sess_1",
                "workspace": {"workspacePath": "/w", "workspaceIdentity": "remote:ssh:host"}}}),
        );
        assert!(drafts.has("/w", Some("remote:ssh:host"), "sess_1"));
        assert!(!drafts.has("/w", Some("/w"), "sess_1"));
        assert_eq!(drafts.len(), 1);

        // 快照没有 identity 时回退请求参数；空白 identity 视同未提供。
        drafts.remember(
            &json!({"workspacePath": "/w2", "workspaceIdentity": "   "}),
            &json!({"session": {"sessionId": "sess_2", "workspace": {"workspacePath": "/w2"}}}),
        );
        assert!(drafts.has("/w2", None, "sess_2"));
        assert!(drafts.has("/w2", Some("  "), "sess_2"));

        drafts.forget("/w2", None, "sess_2");
        assert!(!drafts.has("/w2", None, "sess_2"));
        assert_eq!(drafts.len(), 1);
    }
}
