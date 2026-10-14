//! 任务 → workspace 目标注册表（TS `escodeTaskServiceAdapter` 的 `taskTargets` 同构）。
//!
//! 只有 `{taskId}` 入参的方法（`getTaskConfigOptions`/`getTaskModelSelection`/`setModel`…）
//! 靠它把 taskId 解析回 workspace。它是 **Host 进程内**的状态：重启后需要一次带 workspace
//! 信息的调用（列表读/快照读/创建）重新填充——TS 侧同样如此。
//!
//! 两条写入路径的语义不同（照抄 TS，不能合并）：
//! - `remember`（`rememberTaskTarget`）：**保留**既有 `cronAutomationId`（快照/恢复入口不带该标记）；
//! - `remember_indexed_meta`（`rememberIndexedTaskMeta`）：以索引 meta 为**权威**，
//!   直接覆盖（含清空）cron 归属，供自动化解绑/删除后真正生效。
//!
//! 未命中时报 `ESCODE_SESSION_TARGET_NOT_FOUND`（TS 同 code、同文案）。

use std::collections::HashMap;
use std::sync::Mutex;

/// 一个任务目标（TS `TaskTarget`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTarget {
    pub task_id: String,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub cron_automation_id: Option<String>,
    pub remote_session_id: Option<String>,
}

/// 注册表（`Mutex` 保护；读取路径都在阻塞/短临界区内）。
#[derive(Default)]
pub struct TaskTargets {
    entries: Mutex<HashMap<String, TaskTarget>>,
}

impl TaskTargets {
    pub fn new() -> Self {
        Self::default()
    }

    /// TS `rememberTaskTarget`：合并写入，`cronAutomationId` 缺省保留既有值。
    /// 快照/恢复/创建入口用它（`getTaskSnapshot`/`createTask` 等切片落地后接入）。
    #[allow(dead_code)]
    pub fn remember(&self, mut target: TaskTarget) {
        let mut entries = self.lock();
        let existing = entries.get(&target.task_id);
        if target.cron_automation_id.is_none() {
            target.cron_automation_id =
                existing.and_then(|current| current.cron_automation_id.clone());
        }
        entries.insert(target.task_id.clone(), target);
    }

    /// TS `rememberIndexedTaskMeta`：索引 meta 是权威，含清空 cron 归属。
    pub fn remember_indexed_meta(
        &self,
        task_id: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        cron_automation_id: Option<&str>,
    ) {
        let mut entries = self.lock();
        let existing = entries.get(task_id).cloned();
        entries.insert(
            task_id.to_owned(),
            TaskTarget {
                task_id: task_id.to_owned(),
                workspace_path: workspace_path.to_owned(),
                workspace_identity: workspace_identity
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                cron_automation_id: cron_automation_id.map(str::to_owned),
                remote_session_id: existing.and_then(|current| current.remote_session_id),
            },
        );
    }

    /// TS `getTaskTarget`：未加载时报结构化错误（`ESCODE_SESSION_TARGET_NOT_FOUND`）。
    pub fn get(&self, task_id: &str) -> Result<TaskTarget, (String, String)> {
        self.lock().get(task_id).cloned().ok_or_else(|| {
            (
                "ESCODE_SESSION_TARGET_NOT_FOUND".to_owned(),
                format!("ESCode session target is not loaded: {task_id}"),
            )
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, TaskTarget>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 从一行 meta（`getTaskMeta`/列表项）取出注册表所需字段。
pub fn remember_from_meta(targets: &TaskTargets, meta: &serde_json::Value) {
    let Some(task_id) = meta["taskId"].as_str() else {
        return;
    };
    targets.remember_indexed_meta(
        task_id,
        meta["workspacePath"].as_str().unwrap_or_default(),
        meta["workspaceIdentity"].as_str(),
        meta["cronAutomationId"].as_str(),
    );
}

#[cfg(test)]
#[path = "task_targets_tests.rs"]
mod tests;
