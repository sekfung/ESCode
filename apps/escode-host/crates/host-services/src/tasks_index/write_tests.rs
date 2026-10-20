use super::test_support::{expected_ledger, fixture};
use super::*;
use serde_json::json;

fn write_meta(task_id: &str, updated_at: i64) -> serde_json::Map<String, Value> {
    serde_json::json!({
        "taskId": task_id,
        "traceId": format!("escode-{task_id}"),
        "title": format!("标题 {task_id}"),
        "workspacePath": "/w",
        "createdAt": 10,
        "updatedAt": updated_at,
        "mode": "build",
    })
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn seed_task_meta_if_missing_writes_once_and_keeps_existing() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    let index = TasksIndex::open(&path).unwrap();

    let seeded = index
        .seed_task_meta_if_missing(&write_meta("task_w1", 100))
        .unwrap();
    assert_eq!(seeded["taskId"], "task_w1");
    assert_eq!(seeded["title"], "标题 task_w1");

    // 已存在：不覆盖（标题保持库内值）。
    let mut changed = write_meta("task_w1", 500);
    changed.insert("title".into(), json!("新标题"));
    let again = index.seed_task_meta_if_missing(&changed).unwrap();
    assert_eq!(again["title"], "标题 task_w1", "seed 不覆盖既有行");
    assert_eq!(again["updatedAt"], 100);
}

#[test]
fn sync_task_meta_applies_the_merge_rules() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    let index = TasksIndex::open(&path).unwrap();

    index
        .sync_task_meta(&write_meta("task_w2", 200), &write::SyncOptions::default())
        .unwrap();
    // 用户改过标题 + 终态。
    let mut completed = write_meta("task_w2", 300);
    completed.insert("status".into(), json!("completed"));
    index
        .sync_task_meta(
            &completed,
            &write::SyncOptions {
                title_overridden: Some(true),
                ..write::SyncOptions::default()
            },
        )
        .unwrap();

    // 更旧的 running 快照：标题保留用户值、状态不降级、updatedAt 不回退。
    let mut stale = write_meta("task_w2", 250);
    stale.insert("title".into(), json!("agent 标题"));
    stale.insert("status".into(), json!("running"));
    let merged = index
        .sync_task_meta(&stale, &write::SyncOptions::default())
        .unwrap();
    assert_eq!(merged["title"], "标题 task_w2", "用户标题不被覆盖");
    assert_eq!(merged["titleOverridden"], true);
    assert_eq!(merged["status"], "completed", "终态不被旧 running 降级");
    assert_eq!(merged["updatedAt"], 300, "updatedAt 单调不回退");

    // 身份字段缺省保留既有值。
    let mut with_identity = write_meta("task_w2", 400);
    with_identity.insert("cronAutomationId".into(), json!("cron_1"));
    index
        .sync_task_meta(&with_identity, &write::SyncOptions::default())
        .unwrap();
    let after = index
        .sync_task_meta(&write_meta("task_w2", 450), &write::SyncOptions::default())
        .unwrap();
    assert_eq!(after["cronAutomationId"], "cron_1");

    // searchable_text 只在显式传入时更新。
    let search_result = index
        .query_task_list(&query::TaskListQuery {
            kind: "timeline",
            workspace_scopes: &[query::WorkspaceScope {
                workspace_path: "/w",
                workspace_identity: None,
                workspace_purpose: None,
            }],
            sort_by: "updated",
            search: Some("cron"),
            limit: None,
            provider: None,
        })
        .unwrap();
    assert!(
        search_result["items"].as_array().unwrap().is_empty(),
        "正文尚未写入时不命中搜索"
    );
    index
        .sync_task_meta(
            &write_meta("task_w2", 460),
            &write::SyncOptions {
                searchable_text: Some("正文里有 cron 关键词".to_owned()),
                ..write::SyncOptions::default()
            },
        )
        .unwrap();
    // 后续不带正文的写入不得清空已索引文本。
    index
        .sync_task_meta(&write_meta("task_w2", 470), &write::SyncOptions::default())
        .unwrap();
    let hits_result = index
        .query_task_list(&query::TaskListQuery {
            kind: "timeline",
            workspace_scopes: &[query::WorkspaceScope {
                workspace_path: "/w",
                workspace_identity: None,
                workspace_purpose: None,
            }],
            sort_by: "updated",
            search: Some("cron"),
            limit: None,
            provider: None,
        })
        .unwrap();
    let hits: Vec<&str> = hits_result["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["taskId"].as_str().unwrap())
        .collect();
    assert_eq!(hits, vec!["task_w2"], "已索引正文保留");

    // pinned/archived 缺省沿用既有行。
    index
        .sync_task_meta(
            &write_meta("task_w2", 500),
            &write::SyncOptions {
                pinned: Some(true),
                archived: Some(true),
                ..write::SyncOptions::default()
            },
        )
        .unwrap();
    let flags = index
        .sync_task_meta(&write_meta("task_w2", 510), &write::SyncOptions::default())
        .unwrap();
    let row = index.get_task_meta("/w", None, "task_w2").unwrap().unwrap();
    assert_eq!(row["taskId"], "task_w2");
    assert_eq!(flags["titleOverridden"], true);
    let _ = row;
    let connection = Connection::open(&path).unwrap();
    let (pinned, archived): (i64, i64) = connection
        .query_row(
            "SELECT pinned, archived FROM tasks WHERE task_id = 'task_w2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((pinned, archived), (1, 1), "flags 缺省沿用既有行");
}

#[test]
fn sync_task_meta_respects_the_unread_cas_marker() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    let index = TasksIndex::open(&path).unwrap();
    let mut meta = write_meta("task_unread", 100);
    meta.insert("unreadAt".into(), json!(111));
    index
        .sync_task_meta(&meta, &write::SyncOptions::default())
        .unwrap();

    // 非 unread 写路径：unread_at 保持（CAS marker 不被普通写清掉）。
    let fresh = write_meta("task_unread", 200);
    index
        .sync_task_meta(&fresh, &write::SyncOptions::default())
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    let (unread_at, last_unread_at): (Option<i64>, i64) = connection
        .query_row(
            "SELECT unread_at, last_unread_at FROM tasks WHERE task_id = 'task_unread'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(unread_at, Some(111), "普通写不改 unread_at");
    assert_eq!(last_unread_at, 111, "last_unread_at 记录历史最大值");
}
