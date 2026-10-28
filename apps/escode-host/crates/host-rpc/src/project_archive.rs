//! 任务自动归档的唯一触发者（上游 `project/autoArchiveScheduler.ts`）。
//!
//! 归档判定**不能**挂在 readTree/点击路径上：历史 bug 是点开旧任务 → 列表刷新 → 读取顺手
//! 归档了刚失去未读的那条 → 用户感知「点了打不开」。这里改为 Host 定时扫描 + 设置变更补扫。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::domains_task_support::ReadContext;
use crate::task_index_syncer::TaskIndexSyncer;
use crate::workspace_events::{Target, WorkspaceEvents};

/// 扫描间隔：刚超期或刚失去保护的任务很快离开列表，又不会像挂在点击上那样「点开即消失」。
pub(crate) const SCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);
const POLICY_KEYS: [&str; 2] = ["taskAutoArchiveEnabled", "taskAutoArchiveOlderThanDays"];

/// 设置变更后是否需要补扫。
pub(crate) fn should_rescan(keys: &[String]) -> bool {
    keys.iter().any(|key| POLICY_KEYS.contains(&key.as_str()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// 读取策略；关闭、尚未完成重置迁移或读不到返回 `None`（fail-closed，与上游一致）。
async fn read_policy(home: &Path) -> Option<(i64, u64)> {
    let settings = escode_host_services::settings::read(home).await.ok()?;
    // 与 TS `migrateTaskAutoArchiveReset` 同义：旧版开启过但未执行归档的设置在升级时先关闭，
    // 否则首次扫描会突然归档历史会话。标记在设置写面持久化后不再重置。
    if settings["taskAutoArchiveResetVersion"].as_i64() != Some(1) {
        return None;
    }
    if settings["taskAutoArchiveEnabled"].as_bool() != Some(true) {
        return None;
    }
    let days = settings["taskAutoArchiveOlderThanDays"]
        .as_i64()
        .map_or(30, |days| days.max(1))
        .min(365);
    Some((now_ms() - days * 86_400_000, days as u64))
}

async fn scan(read: &ReadContext, active: Vec<Value>, home: &Path) -> Vec<Value> {
    let Some((cutoff, _days)) = read_policy(home).await else {
        return Vec::new();
    };
    let input = json!({"cutoff": cutoff, "activeTasks": active});
    match read
        .run(move |index| {
            index
                .project_archive_inactive(&input)
                .map_err(|error| error.to_string())
        })
        .await
    {
        Ok(archived) => archived,
        Err((_, error)) => {
            let line = format!("自动归档扫描失败，等待下一轮: {error}");
            escode_host_control::logging::log(
                "task-auto-archive",
                escode_host_control::LogLevel::Warn,
                vec![&line],
            );
            Vec::new()
        }
    }
}

/// 启动常驻扫描：首轮立即执行，之后固定间隔；`notify` 在自动归档设置变更后唤醒补扫。
///
/// 没有 tokio runtime 的嵌入场景（同步契约测试）不启动扫描，返回 `false`。
pub(crate) fn spawn(
    read: ReadContext,
    syncer: Arc<TaskIndexSyncer>,
    events: Arc<WorkspaceEvents>,
    home: PathBuf,
    notify: Arc<Notify>,
) -> bool {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    handle.spawn(async move {
        let mut interval = tokio::time::interval(SCAN_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = notify.notified() => {}
            }
            // 活动判定用 syncer 已观察到的非终态会话（TS `activeTaskRefs`）；扫描不启动运行时。
            for task in scan(&read, syncer.active_task_refs(), &home).await {
                let path = task["workspacePath"].as_str().unwrap_or_default();
                events.task_list_changed(
                    &Target {
                        workspace_path: path,
                        workspace_identity: task["workspaceIdentity"].as_str(),
                        task_id: task["taskId"].as_str(),
                    },
                    None,
                    "archive",
                    None,
                );
            }
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains_task_support::{LazyIndex, ReadContext};
    use crate::task_targets::TaskTargets;
    use std::sync::Mutex;

    #[test]
    fn only_auto_archive_settings_trigger_a_rescan() {
        assert!(should_rescan(&["taskAutoArchiveEnabled".to_owned()]));
        assert!(should_rescan(&[
            "theme".to_owned(),
            "taskAutoArchiveOlderThanDays".to_owned()
        ]));
        assert!(!should_rescan(&["theme".to_owned()]));
    }

    fn seed(root: &Path) {
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let path = crate::domains_task_support::tasks_index_path(&root.join("data"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        escode_host_services::tasks_index::fixture::apply_generated_schema(
            &connection,
            escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS,
        )
        .unwrap();
        let old = now_ms() - 30 * 86_400_000;
        for (task_id, status) in [("stale", "completed"), ("running", "running")] {
            connection
                .execute(
                    "INSERT INTO tasks(workspace_key,workspace_path,task_id,title,mode,created_at,updated_at,
                       host_id,root_path,cli_created_at,cli_updated_at,task_status)
                     VALUES(?1,?1,?2,'任务','build',?3,?3,'local',?1,?3,?3,?4)",
                    rusqlite::params!["/w", task_id, old, status],
                )
                .unwrap();
        }
    }

    fn context(root: &Path) -> ReadContext {
        ReadContext {
            path: crate::domains_task_support::tasks_index_path(&root.join("data")),
            index: Arc::new(Mutex::new(LazyIndex::default())),
            targets: Arc::new(TaskTargets::new()),
            runtime: None,
        }
    }

    #[tokio::test]
    async fn scan_archives_stale_tasks_but_never_running_ones() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let home = root.path().join("home");
        escode_host_services::settings::apply_patch(
            &home,
            &json!({
                "taskAutoArchiveEnabled": true,
                "taskAutoArchiveResetVersion": 1,
                "taskAutoArchiveOlderThanDays": 7,
            }),
        )
        .await
        .unwrap();

        // syncer 观察到的运行中会话（TS `activeTaskRefs`）。
        let active = vec![json!({"workspacePath": "/w", "taskId": "running"})];
        let archived = scan(&context(root.path()), active, &home).await;
        let ids: Vec<&str> = archived
            .iter()
            .filter_map(|task| task["taskId"].as_str())
            .collect();
        assert_eq!(ids, vec!["stale"]);

        let connection = rusqlite::Connection::open(crate::domains_task_support::tasks_index_path(
            &root.path().join("data"),
        ))
        .unwrap();
        let stale: i64 = connection
            .query_row(
                "SELECT archived FROM tasks WHERE task_id='stale'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let running: i64 = connection
            .query_row(
                "SELECT archived FROM tasks WHERE task_id='running'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((stale, running), (1, 0));
    }

    #[tokio::test]
    async fn disabled_policy_archives_nothing() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let home = root.path().join("home");
        escode_host_services::settings::apply_patch(
            &home,
            &json!({
                "taskAutoArchiveEnabled": false,
                "taskAutoArchiveResetVersion": 1,
                "taskAutoArchiveOlderThanDays": 7,
            }),
        )
        .await
        .unwrap();
        assert!(
            scan(&context(root.path()), Vec::new(), &home)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn settings_without_the_reset_marker_archive_nothing() {
        let root = tempfile::tempdir().unwrap();
        seed(root.path());
        let home = root.path().join("home");
        // 旧版遗留：开关开启但没有重置标记，升级后首次读取必须先关闭。
        escode_host_services::settings::apply_patch(
            &home,
            &json!({"taskAutoArchiveEnabled": true, "taskAutoArchiveOlderThanDays": 7}),
        )
        .await
        .unwrap();
        assert!(
            scan(&context(root.path()), Vec::new(), &home)
                .await
                .is_empty()
        );
    }
}
