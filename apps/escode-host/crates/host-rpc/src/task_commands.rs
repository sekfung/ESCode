//! `escode-task` 手机 replayable 的 Host 命令队列（S8，TS `escodeTaskServiceAdapter` 的
//! `runtimeCommands` / `activePromptInputIds` / `scheduleRuntimeCommandDrain` 同构）。
//!
//! 状态所有者是 Host 进程内存（不落库、不跨重启）；键 = 身份键 + `\0` + taskId。
//! 活跃轮次由 `sendPrompt`/`compactSession` 写入，由 sessions-index 的 phase 终态迁移清除；
//! 终态后 syncer 调 [`TaskCommandQueue::session_terminal`] 收口当前命令并放行下一条。
//!
//! 登记差异（docs/specs/rust-u2-tasks-index.md §13.4）：TS 取消时会发 `task_snapshot_updated`
//! 任务事件；Rust Host 的任务事件发射器随事件面一起迁移，本片不新增部分实现。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use escode_host_control::logging;
use escode_host_services::tasks_index::workspace_key;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::domains_task_state::StateContext;
use crate::session::RpcError;
use crate::task_targets::TaskTarget;

#[derive(Default)]
struct QueueState {
    /// 任务键 → 命令（保持入队/提升后的顺序）。
    commands: HashMap<String, Vec<Value>>,
    /// 任务键 → 当前轮次 inputId（V4 commandId）。
    active: HashMap<String, String>,
}

#[derive(Default)]
pub(crate) struct TaskCommandQueue {
    state: Mutex<QueueState>,
    /// drain 执行上下文（`task_channel` 装配时注入；单元测试不注入 = 只验证状态机）。
    context: OnceLock<StateContext>,
    /// 每任务键一把异步互斥锁：drain 串行，`session_terminal` 的调度不会并发发送。
    drains: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn task_key(target: &TaskTarget) -> String {
    format!(
        "{}\0{}",
        workspace_key(&target.workspace_path, target.workspace_identity.as_deref()),
        target.task_id
    )
}

fn target_for(workspace_path: &str, identity: Option<&str>, task_id: &str) -> TaskTarget {
    TaskTarget {
        task_id: task_id.to_owned(),
        workspace_path: workspace_path.to_owned(),
        workspace_identity: identity.map(str::to_owned),
        cron_automation_id: None,
        remote_session_id: None,
    }
}

fn owner_error(message: &str, code: &str) -> RpcError {
    RpcError::new("Error", message).with("code", Value::from(code))
}

impl TaskCommandQueue {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn attach(&self, context: StateContext) {
        let _ = self.context.set(context);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn set_active(&self, target: &TaskTarget, input_id: &str) {
        self.lock()
            .active
            .insert(task_key(target), input_id.to_owned());
    }

    pub(crate) fn clear_active(&self, target: &TaskTarget) {
        self.lock().active.remove(&task_key(target));
    }

    fn assert_owner(
        &self,
        target: &TaskTarget,
        owner_run_id: Option<&str>,
    ) -> Result<(), RpcError> {
        let Some(owner_run_id) = owner_run_id.filter(|value| !value.is_empty()) else {
            return Ok(());
        };
        let state = self.lock();
        let key = task_key(target);
        let Some(active) = state.active.get(&key) else {
            return Err(owner_error("No active task owner.", "NO_ACTIVE_TASK_OWNER"));
        };
        if active != owner_run_id {
            return Err(owner_error(
                "Stale task owner command.",
                "STALE_TASK_OWNER_COMMAND",
            ));
        }
        Ok(())
    }

    pub(crate) fn pending(&self, target: &TaskTarget) -> Vec<Value> {
        self.lock()
            .commands
            .get(&task_key(target))
            .cloned()
            .unwrap_or_default()
    }

    /// TS `enqueueTaskCommand`：入队后立即调度 drain（active 存在时 drain 自行退出）。
    pub(crate) fn enqueue(&self, target: &TaskTarget, params: &Value) -> Result<Value, RpcError> {
        self.assert_owner(target, params["ownerRunId"].as_str())?;
        let now = now_ms();
        let command = command_of(target, params, now);
        let key = task_key(target);
        self.lock()
            .commands
            .entry(key)
            .or_default()
            .push(command.clone());
        Ok(json!({"accepted": true, "command": command}))
    }

    /// TS `promoteTaskCommand`：running 原样返回；其余置 accepted 并移到队首。
    pub(crate) fn promote(&self, target: &TaskTarget, params: &Value) -> Result<Value, RpcError> {
        self.assert_owner(target, params["ownerRunId"].as_str())?;
        let command_id = params["commandId"].as_str().unwrap_or_default();
        let key = task_key(target);
        let mut state = self.lock();
        let Some(commands) = state.commands.get_mut(&key) else {
            return Err(not_found());
        };
        let Some(position) = commands
            .iter()
            .position(|item| item["commandId"].as_str() == Some(command_id))
        else {
            return Err(not_found());
        };
        if commands[position]["status"] == "running" {
            return Ok(json!({"accepted": true, "command": commands[position]}));
        }
        let mut promoted = commands.remove(position);
        promoted["status"] = Value::from("accepted");
        promoted["updatedAt"] = Value::from(now_ms());
        commands.insert(0, promoted.clone());
        Ok(json!({"accepted": true, "command": promoted}))
    }

    /// TS `cancelTaskCommand`：不存在/运行中都是幂等结果，只有 accepted/failed 才真正删除。
    pub(crate) fn cancel(&self, target: &TaskTarget, params: &Value) -> Result<Value, RpcError> {
        self.assert_owner(target, params["ownerRunId"].as_str())?;
        let command_id = params["commandId"].as_str().unwrap_or_default();
        let key = task_key(target);
        let mut state = self.lock();
        let Some(commands) = state.commands.get_mut(&key) else {
            return Ok(not_found_result(command_id));
        };
        let Some(position) = commands
            .iter()
            .position(|item| item["commandId"].as_str() == Some(command_id))
        else {
            return Ok(not_found_result(command_id));
        };
        let command = commands[position].clone();
        if command["status"] == "running" {
            logging::info(
                "task-command",
                format!("cancel skipped: command already running commandId={command_id}"),
            );
            return Ok(json!({
                "canceled": false,
                "commandId": command_id,
                "reason": "already_running",
                "status": command["status"],
            }));
        }
        commands.remove(position);
        if commands.is_empty() {
            state.commands.remove(&key);
        }
        logging::info(
            "task-command",
            format!(
                "command canceled commandId={command_id} status={}",
                command["status"].as_str().unwrap_or_default()
            ),
        );
        Ok(json!({
            "canceled": true,
            "commandId": command_id,
            "status": command["status"],
        }))
    }

    /// syncer 观察到 phase 终态：收口当前 running 命令、清 active，并放行下一条。
    pub(crate) fn session_terminal(
        self: &Arc<Self>,
        workspace_path: &str,
        identity: Option<&str>,
        task_id: &str,
    ) -> Option<String> {
        let target = target_for(workspace_path, identity, task_id);
        let key = task_key(&target);
        let (input_id, has_commands) = {
            let mut state = self.lock();
            let input_id = state.active.remove(&key);
            if let Some(active) = input_id.as_ref()
                && let Some(commands) = state.commands.get_mut(&key)
            {
                commands.retain(|item| {
                    !(item["type"] == "send_prompt"
                        && item["status"] == "running"
                        && item["traceId"].as_str() == Some(active.as_str()))
                });
                if commands.is_empty() {
                    state.commands.remove(&key);
                }
            }
            (input_id, state.commands.contains_key(&key))
        };
        if has_commands {
            self.schedule(target, "session-ready");
        }
        input_id
    }

    /// 取走本轮可发的命令并置 running（同一把锁内完成，避免两次 drain 抢同一条）。
    fn take_next(&self, target: &TaskTarget) -> Option<Value> {
        let key = task_key(target);
        let mut state = self.lock();
        if state.active.contains_key(&key) {
            return None;
        }
        let commands = state.commands.get_mut(&key)?;
        if commands.iter().any(|item| item["status"] == "running") {
            return None;
        }
        let position = commands
            .iter()
            .position(|item| item["type"] == "send_prompt" && item["status"] == "accepted")?;
        let mut running = commands[position].clone();
        running["status"] = Value::from("running");
        running["updatedAt"] = Value::from(now_ms());
        commands[position] = running.clone();
        Some(running)
    }

    fn mark_failed(&self, target: &TaskTarget, command_id: &str, error: &str) {
        let key = task_key(target);
        let mut state = self.lock();
        let Some(commands) = state.commands.get_mut(&key) else {
            return;
        };
        for item in commands.iter_mut() {
            if item["commandId"].as_str() != Some(command_id) {
                continue;
            }
            item["status"] = Value::from("failed");
            item["updatedAt"] = Value::from(now_ms());
            item["error"] = Value::from(error);
        }
    }

    /// 调度一次 drain；每任务键一把异步锁，等待者按到达序执行（TS drain 链同义）。
    pub(crate) fn schedule(self: &Arc<Self>, target: TaskTarget, reason: &'static str) {
        let Some(context) = self.context.get().cloned() else {
            return;
        };
        let key = task_key(&target);
        let drain_lock = {
            let mut drains = self
                .drains
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                drains
                    .entry(key)
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let queue = Arc::clone(self);
        tokio::spawn(async move {
            let _guard = drain_lock.lock().await;
            let Some(command) = queue.take_next(&target) else {
                return;
            };
            let command_id = command["commandId"].as_str().unwrap_or_default().to_owned();
            logging::info(
                "task-command",
                format!(
                    "drain started commandId={command_id} reason={reason} taskId={}",
                    target.task_id
                ),
            );
            if let Err(error) =
                crate::domains_task_send::deliver_task_command(&context, &target, &command).await
            {
                queue.mark_failed(&target, &command_id, &error.message);
                logging::warn(
                    "task-command",
                    format!(
                        "drain failed commandId={command_id} taskId={}: {}",
                        target.task_id, error.message
                    ),
                );
            }
        });
    }
}

fn not_found() -> RpcError {
    owner_error("Task command not found.", "OWNER_COMMAND_FAILED")
}

fn not_found_result(command_id: &str) -> Value {
    logging::info(
        "task-command",
        format!("cancel skipped: command not found commandId={command_id}"),
    );
    json!({"canceled": true, "commandId": command_id, "reason": "not_found"})
}

/// TS `enqueueTaskCommand` 的命令字面量：`undefined` 字段整键省略。
fn command_of(target: &TaskTarget, params: &Value, now: i64) -> Value {
    let mut command = Map::new();
    let mut put = |key: &str, value: Value| {
        command.insert(key.to_owned(), value);
    };
    for key in ["commandId", "traceId", "queryId", "clientId", "clientLabel"] {
        if let Some(value) = params[key].as_str() {
            put(key, Value::from(value));
        }
    }
    put("taskId", Value::from(target.task_id.clone()));
    put("workspacePath", Value::from(target.workspace_path.clone()));
    if let Some(identity) = target.workspace_identity.as_ref() {
        put("workspaceIdentity", Value::from(identity.clone()));
    }
    put(
        "workspaceKey",
        Value::from(workspace_key(
            &target.workspace_path,
            target.workspace_identity.as_deref(),
        )),
    );
    put("status", Value::from("accepted"));
    put("createdAt", Value::from(now));
    put("updatedAt", Value::from(now));
    put("type", Value::from("send_prompt"));
    put(
        "content",
        Value::from(params["content"].as_str().unwrap_or_default()),
    );
    if let Some(attachments) = params["attachments"].as_array() {
        put("attachments", Value::Array(attachments.clone()));
    }
    if let Some(automation) = params["automationId"].as_str() {
        put("automationId", Value::from(automation));
    }
    Value::Object(command)
}

#[cfg(test)]
#[path = "task_commands_tests.rs"]
mod tests;
