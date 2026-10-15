//! Host 级 workspace 事件总线（TS `escodeTaskIndexSyncer.ts` 的 `getWorkspaceEmitter` /
//! `emitWorkspaceTaskListChanged`，经 `escode-task.onDynamicWorkspaceEvent` 投递给 Renderer）。
//!
//! 键 = workspace 身份键（`workspaceIdentity?.trim() || workspacePath`）。监听不代表用户在用该
//! workspace：**只登记监听，不启动 runtime、不建任何订阅**（TS 同：侧栏为全部 restored workspace 监听）。
//!
//! 溢出策略：监听者通道无界（事件是低频的列表变更通知，单条很小；每次写操作至多一条）。

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::session::EventStream;
use crate::wire::Wire;

#[derive(Default)]
pub(crate) struct WorkspaceEvents {
    listeners: Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
}

/// 事件定位：TS `WorkspaceBroadcastTarget`。
pub(crate) struct Target<'a> {
    pub workspace_path: &'a str,
    pub workspace_identity: Option<&'a str>,
    pub task_id: Option<&'a str>,
}

/// TS `WorkspaceEventInput`：身份键字符串或 `{workspacePath, workspaceIdentity}`。
pub(crate) fn key_of_input(input: &Value) -> String {
    if let Some(key) = input.as_str() {
        return key.to_owned();
    }
    let path = input["workspacePath"].as_str().unwrap_or_default();
    escode_host_services::tasks_index::workspace_key(path, input["workspaceIdentity"].as_str())
}

impl WorkspaceEvents {
    pub(crate) fn listen(&self, input: &Value) -> EventStream {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().entry(key_of_input(input)).or_default().push(tx);
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|value| (Wire::Object(value), rx))
        }))
    }

    pub(crate) fn emit(&self, workspace_key: &str, event: &Value) {
        let mut listeners = self.lock();
        if let Some(entries) = listeners.get_mut(workspace_key) {
            entries.retain(|tx| tx.send(event.clone()).is_ok());
            if entries.is_empty() {
                listeners.remove(workspace_key);
            }
        }
    }

    /// TS `emitWorkspaceTaskListChanged`：`undefined` 字段整键省略。
    pub(crate) fn task_list_changed(
        &self,
        target: &Target<'_>,
        task_meta: Option<&Value>,
        reason: &str,
        unread_signal: Option<&str>,
    ) {
        let mut event = json!({
            "type": "workspace_task_list_changed",
            "workspacePath": target.workspace_path,
        });
        if let Some(identity) = target.workspace_identity {
            event["workspaceIdentity"] = Value::from(identity);
        }
        if let Some(task_id) = target.task_id {
            event["taskId"] = Value::from(task_id);
        }
        event["reason"] = Value::from(reason);
        if let Some(meta) = task_meta.filter(|meta| !meta.is_null()) {
            event["taskMeta"] = meta.clone();
        }
        if let Some(signal) = unread_signal {
            event["unreadSignal"] = Value::from(signal);
        }
        let key = escode_host_services::tasks_index::workspace_key(
            target.workspace_path,
            target.workspace_identity,
        );
        self.emit(&key, &event);
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>> {
        self.listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn events_reach_listeners_of_the_same_identity_key_only() {
        let events = WorkspaceEvents::default();
        let mut by_object =
            events.listen(&json!({"workspacePath": "/w", "workspaceIdentity": " ssh:host "}));
        let mut by_key = events.listen(&json!("ssh:host"));
        let mut other = events.listen(&json!({"workspacePath": "/w"}));
        events.task_list_changed(
            &Target {
                workspace_path: "/w",
                workspace_identity: Some(" ssh:host "),
                task_id: Some("t1"),
            },
            Some(&json!({"taskId": "t1"})),
            "task_pinned",
            None,
        );
        for stream in [&mut by_object, &mut by_key] {
            let Some(Wire::Object(event)) = stream.next().await else {
                panic!("event")
            };
            assert_eq!(event["type"], "workspace_task_list_changed");
            assert_eq!(event["reason"], "task_pinned");
            assert_eq!(event["taskMeta"]["taskId"], "t1");
            assert!(event.get("unreadSignal").is_none());
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), other.next())
                .await
                .is_err(),
            "path-only key is a different workspace"
        );
    }
}
