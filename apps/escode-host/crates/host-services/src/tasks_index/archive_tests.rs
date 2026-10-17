//! 批量归档（`rust-u2-tasks-index.md` §9）。
use super::super::test_support::{expected_ledger, fixture};
use super::super::write_state;
use super::super::{StaleArchiveParams, SyncOptions, TasksIndex};
use serde_json::{Value, json};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn meta(
    task_id: &str,
    updated_at: i64,
    status: &str,
    provider: &str,
) -> serde_json::Map<String, Value> {
    json!({
        "taskId": task_id,
        "traceId": format!("escode-{task_id}"),
        "title": format!("标题 {task_id}"),
        "workspacePath": "/w",
        "createdAt": 1,
        "updatedAt": updated_at,
        "mode": "build",
        "status": status,
        "provider": provider,
    })
    .as_object()
    .unwrap()
    .clone()
}

fn set_unread(index: &TasksIndex, task_id: &str) {
    index
        .update_task_state(
            &write_state::TaskRef {
                workspace_path: "/w",
                workspace_identity: None,
                task_id,
            },
            &write_state::StatePatch {
                unread_at: write_state::Field::Set(now_ms()),
                ..write_state::StatePatch::default()
            },
        )
        .unwrap();
}

#[test]
fn stale_archive_only_touches_completed_unpinned_unread_free_rows() {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).unwrap();
    let old = now_ms() - 10 * 24 * 60 * 60 * 1000;
    let recent = now_ms() - 60 * 60 * 1000;
    for (task_id, updated_at, status) in [
        ("stale", old, "completed"),
        ("recent", recent, "completed"),
        ("running", old, "running"),
        ("pinned", old, "completed"),
        ("unread", old, "completed"),
        ("archived", old, "completed"),
        ("other", old, "completed"),
    ] {
        let provider = if task_id == "other" { "claude" } else { "glm" };
        index
            .sync_task_meta(
                &meta(task_id, updated_at, status, provider),
                &SyncOptions::default(),
            )
            .unwrap();
    }
    for (task_id, options) in [
        (
            "pinned",
            SyncOptions {
                pinned: Some(true),
                ..SyncOptions::default()
            },
        ),
        (
            "archived",
            SyncOptions {
                archived: Some(true),
                ..SyncOptions::default()
            },
        ),
    ] {
        index
            .sync_task_meta(&meta(task_id, old, "completed", "glm"), &options)
            .unwrap();
    }
    set_unread(&index, "unread");

    // 带 provider 过滤：只归档该 provider 的旧完成行。
    let archived = index
        .archive_stale_tasks(&StaleArchiveParams {
            workspace_path: "/w",
            workspace_identity: None,
            older_than_days: 7,
            provider: Some("claude"),
        })
        .unwrap();
    assert_eq!(archived[0]["taskId"], json!("other"), "{archived:?}");

    // 不带 provider：清理全部历史 provider。
    let archived = index
        .archive_stale_tasks(&StaleArchiveParams {
            workspace_path: "/w",
            workspace_identity: None,
            older_than_days: 7,
            provider: None,
        })
        .unwrap();
    let ids: Vec<&str> = archived
        .iter()
        .filter_map(|meta| meta["taskId"].as_str())
        .collect();
    assert_eq!(ids, vec!["stale"], "{archived:?}");

    // 非正数天数按 1 天处理（不会把所有任务都归档）。
    let archived = index
        .archive_stale_tasks(&StaleArchiveParams {
            workspace_path: "/w",
            workspace_identity: None,
            older_than_days: 0,
            provider: None,
        })
        .unwrap();
    assert!(archived.is_empty());
}

#[test]
fn workspace_archive_archives_all_glm_tasks_and_returns_the_archived_list() {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).unwrap();
    index
        .sync_task_meta(&meta("a", 10, "completed", "glm"), &SyncOptions::default())
        .unwrap();
    index
        .sync_task_meta(&meta("b", 20, "running", "glm"), &SyncOptions::default())
        .unwrap();
    index
        .sync_task_meta(
            &meta("c", 30, "completed", "claude"),
            &SyncOptions::default(),
        )
        .unwrap();

    let archived = index.archive_workspace_tasks("/w", None).unwrap();
    let ids: Vec<&str> = archived
        .iter()
        .filter_map(|meta| meta["taskId"].as_str())
        .collect();
    assert_eq!(ids, vec!["b", "a"], "只归档 glm，按 updated_at 降序返回");

    // 再归档一次：列表不变（幂等）。
    let again = index.archive_workspace_tasks("/w", None).unwrap();
    assert_eq!(again.len(), 2);
}
