//! `escode-task` 任务事件总线（S11，TS `getTaskEmitter` / syncer terminal/ready 同构）。
//!
//! 事件源只有 Host 自己：发送开始（`task_run_started`）、sessions-index 终态迁移
//! （`task_complete`/`task_error` + `task_snapshot_updated`）、以及 ready outcome。
//! V4 conversation 帧 → `ESCodeStreamEvent` 的完整映射属后续投影片（spec §16 登记差异）。
//!
//! 键 = taskId（TS `taskEmitters` 同键）；监听者无界，Host 只发低频生命周期事件。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::session::{EventHandler, EventStream};
use crate::task_targets::TaskTarget;
use crate::wire::Wire;

#[derive(Default)]
pub(crate) struct TaskEventHub {
    streams: Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
    ready: Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
    terminal: Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
    errors: Mutex<Vec<mpsc::UnboundedSender<Value>>>,
}

impl TaskEventHub {
    pub(crate) fn listen_stream(&self, task_id: &str) -> EventStream {
        stream_of(listen(&self.streams, task_id))
    }

    pub(crate) fn listen_ready(&self, task_id: &str) -> EventStream {
        stream_of(listen(&self.ready, task_id))
    }

    pub(crate) fn listen_terminal(&self, task_id: &str) -> EventStream {
        stream_of(listen(&self.terminal, task_id))
    }

    /// `onError`：Host 尚无统一错误总线，监听存在但当前不投递（spec §16 登记差异）。
    pub(crate) fn listen_errors(&self) -> EventStream {
        let (tx, rx) = mpsc::unbounded_channel();
        self.errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(tx);
        stream_of(rx)
    }

    /// TS `turn.started` 的 Host 侧同构：命令已提交给运行时，登记本轮 inputId。
    pub(crate) fn run_started(&self, target: &TaskTarget, input_id: &str) {
        send(
            &self.streams,
            &target.task_id,
            json!({
                "type": "task_run_started",
                "taskId": target.task_id,
                "traceId": input_id,
                "inputId": input_id,
                "startedAt": now_ms(),
            }),
        );
    }

    /// syncer phase 终态：投 `task_complete/error`、快照通知、ready 与 terminal outcome。
    pub(crate) fn terminal(
        &self,
        workspace_path: &str,
        identity: Option<&str>,
        task_id: &str,
        input_id: Option<&str>,
        error: Option<&str>,
    ) {
        let trace_id = input_id.unwrap_or(task_id);
        let failed = error.is_some();
        let mut event = if let Some(error) = error {
            json!({
                "type": "task_error",
                "taskId": task_id,
                "traceId": trace_id,
                "error": error,
            })
        } else {
            json!({
                "type": "task_complete",
                "taskId": task_id,
                "traceId": trace_id,
                "stopReason": "end_turn",
            })
        };
        if let Some(input_id) = input_id {
            event["inputId"] = Value::from(input_id);
        }
        send(&self.streams, task_id, event);
        let mut snapshot = json!({
            "type": "task_snapshot_updated",
            "workspacePath": workspace_path,
            "workspaceKey": escode_host_services::tasks_index::workspace_key(workspace_path, identity),
            "taskId": task_id,
            "traceId": trace_id,
            "reason": "task_status_changed",
        });
        if let Some(identity) = identity {
            snapshot["workspaceIdentity"] = Value::from(identity);
        }
        send(&self.streams, task_id, snapshot);
        send(
            &self.ready,
            task_id,
            json!({
                "taskId": task_id,
                "reason": if failed { "prompt_failed" } else { "prompt_completed" },
            }),
        );
        let mut outcome = json!({
            "taskId": task_id,
            "outcome": if failed { "failed" } else { "succeeded" },
        });
        if let Some(input_id) = input_id {
            outcome["inputId"] = Value::from(input_id);
        }
        send(&self.terminal, task_id, outcome);
    }

    /// `cancelTaskCommand` 等 Host 写面的快照刷新通知。
    pub(crate) fn snapshot_updated(&self, target: &TaskTarget, trace_id: &str) {
        let mut event = json!({
            "type": "task_snapshot_updated",
            "workspacePath": target.workspace_path,
            "workspaceKey": escode_host_services::tasks_index::workspace_key(
                &target.workspace_path,
                target.workspace_identity.as_deref(),
            ),
            "taskId": target.task_id,
            "traceId": trace_id,
            "reason": "task_status_changed",
        });
        if let Some(identity) = target.workspace_identity.as_ref() {
            event["workspaceIdentity"] = Value::from(identity.clone());
        }
        send(&self.streams, &target.task_id, event);
    }

    /// 事件方法表（追加到 `escode-task` channel）。
    pub(crate) fn handlers(self: &Arc<Self>) -> Vec<(&'static str, EventHandler)> {
        let stream = {
            let hub = Arc::clone(self);
            Arc::new(move |arg: Wire| {
                let task_id = task_id_of(arg);
                Ok(hub.listen_stream(&task_id))
            }) as EventHandler
        };
        let ready = {
            let hub = Arc::clone(self);
            Arc::new(move |arg: Wire| {
                let task_id = task_id_of(arg);
                Ok(hub.listen_ready(&task_id))
            }) as EventHandler
        };
        let terminal = {
            let hub = Arc::clone(self);
            Arc::new(move |arg: Wire| {
                let task_id = task_id_of(arg);
                Ok(hub.listen_terminal(&task_id))
            }) as EventHandler
        };
        let errors = {
            let hub = Arc::clone(self);
            Arc::new(move |_arg: Wire| Ok(hub.listen_errors())) as EventHandler
        };
        vec![
            ("onDynamicStreamEvent", Arc::clone(&stream)),
            ("onDynamicTaskEvent", stream),
            ("onDynamicTaskReady", ready),
            ("onDynamicTaskTerminalOutcome", terminal),
            ("onError", errors),
        ]
    }
}

fn listen(
    map: &Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
    key: &str,
) -> mpsc::UnboundedReceiver<Value> {
    let (tx, rx) = mpsc::unbounded_channel();
    map.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(key.to_owned())
        .or_default()
        .push(tx);
    rx
}

fn stream_of(receiver: mpsc::UnboundedReceiver<Value>) -> EventStream {
    Box::pin(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move {
            receiver
                .recv()
                .await
                .map(|value| (Wire::Object(value), receiver))
        },
    ))
}

fn send(map: &Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>, key: &str, event: Value) {
    let mut listeners = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entries) = listeners.get_mut(key) {
        entries.retain(|tx| tx.send(event.clone()).is_ok());
        if entries.is_empty() {
            listeners.remove(key);
        }
    }
}

fn task_id_of(arg: Wire) -> String {
    let first = arg
        .into_args()
        .into_iter()
        .next()
        .unwrap_or(Wire::Undefined);
    match first {
        Wire::Str(value) => value,
        other => other
            .into_object()
            .and_then(|object| object["taskId"].as_str().map(str::to_owned))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
#[path = "task_events_tests.rs"]
mod tests;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
