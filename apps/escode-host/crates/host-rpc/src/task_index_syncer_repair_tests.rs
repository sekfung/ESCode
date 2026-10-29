//! `repairSubagentTaskIndex` 的 Host 侧回归：旧索引里的 `subagent_child` 存量行只按 CLI
//! 权威身份软删除，可见行与普通会话行保留；重复清扫幂等。

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;

use super::tests::{index_wire, next_event, snapshot_payload, summary, wait_for};
use crate::agent_v4::base::Base;
use crate::agent_v4::frames::TopicKind;
use crate::domains_task_support::{LazyIndex, ReadContext};
use crate::forward::RuntimePolicy;
use crate::task_index_syncer::{Ingest, Workspace};
use crate::task_targets::TaskTargets;
use crate::workspace_events::WorkspaceEvents;

use crate::task_index_syncer::topic as sync_topic;

/// embedded runtime 夹具：CLI 会话库预置旧版 `subagent_child` 行与普通会话行，
/// tasks-index 预置同批存量行，复现「旧版本把 child 写成主列表行」的升级现场。
async fn ingest_with_legacy_child_rows() -> (
    tempfile::TempDir,
    Arc<Ingest>,
    crate::session::EventStream,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let workspace_dir = dir.path().join("ws");
    let data_dir = dir.path().join("data");
    for path in [&home, &workspace_dir, &data_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    let workspace = std::fs::canonicalize(&workspace_dir)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // CLI 权威身份：rust_session.workspace 必须等于 workspace 身份键。
    let cli_db = rusqlite::Connection::open(data_dir.join("rust-sessions.sqlite")).unwrap();
    cli_db
        .execute_batch(
            "CREATE TABLE rust_session(workspace TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(workspace,id));",
        )
        .unwrap();
    for (id, kind) in [
        ("child-stale", "subagent_child"),
        ("child-visible", "subagent_child"),
        ("interactive-stale", "interactive"),
    ] {
        let body = json!({
            "id": id,
            "workspace": workspace,
            "taskType": kind,
            "title": id,
            "titleSource": "model",
            "createdAt": 1,
            "updatedAt": 2,
            "workspacePath": workspace,
            "workspaceDirectory": workspace,
        });
        cli_db
            .execute(
                "INSERT INTO rust_session VALUES(?1,?2,?3)",
                rusqlite::params![workspace, id, body.to_string()],
            )
            .unwrap();
    }
    drop(cli_db);

    let index_dir = dir.path().join("db").join(".escode").join("v2");
    std::fs::create_dir_all(&index_dir).unwrap();
    let index_path = index_dir.join("tasks-index.sqlite");
    let connection = rusqlite::Connection::open(&index_path).unwrap();
    escode_host_services::tasks_index::fixture::apply_generated_schema(
        &connection,
        escode_host_services::tasks_index::schema::EXPECTED_MIGRATIONS,
    )
    .unwrap();
    drop(connection);
    {
        let index = escode_host_services::tasks_index::TasksIndex::open(&index_path).unwrap();
        for task_id in ["child-stale", "child-visible", "interactive-stale"] {
            let meta = json!({
                "taskId": task_id,
                "traceId": task_id,
                "title": task_id,
                "workspacePath": workspace,
                "createdAt": 1,
                "updatedAt": 2,
                "mode": "default",
                "provider": "glm",
            });
            index
                .seed_task_meta_if_missing(meta.as_object().unwrap())
                .unwrap();
        }
    }

    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory = Arc::new({
        let cwd = std::path::PathBuf::from(&workspace);
        move |_descriptor: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: home.clone(),
                env: std::collections::BTreeMap::new(),
                argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
                cwd: cwd.clone(),
                workspace_identity: None,
                data_dir: Some(data_dir.clone()),
                config: None,
                legacy_import: escode_cli_rust::embed::LegacyImport::Disabled,
            }
        }
    });
    let plugin_management =
        crate::domains_plugin::PluginManagement::new(instances.clone(), options.clone(), None);
    let base = Arc::new(Base::new(
        instances,
        options,
        None,
        None,
        plugin_management,
        None,
    ));
    let events = Arc::new(WorkspaceEvents::default());
    let event_stream = events.listen(&json!(workspace));
    let (overlay_changes, _) = tokio::sync::broadcast::channel(8);
    let ingest = Arc::new(Ingest::new(
        base,
        ReadContext {
            path: index_path,
            index: Arc::new(std::sync::Mutex::new(LazyIndex::default())),
            targets: Arc::new(TaskTargets::new()),
            runtime: None,
        },
        events,
        Arc::new(crate::task_commands::TaskCommandQueue::new()),
        Arc::new(crate::task_events::TaskEventHub::default()),
        "test-conn#task-index".to_owned(),
        Workspace {
            path: workspace.clone(),
            identity: None,
            key: workspace.clone(),
        },
        overlay_changes,
    ));
    (dir, ingest, event_stream, workspace)
}

fn deleted_flag(index_path: &std::path::Path, task_id: &str) -> bool {
    let connection = rusqlite::Connection::open(index_path).unwrap();
    connection
        .query_row(
            "SELECT deleted FROM tasks WHERE task_id = ?1",
            rusqlite::params![task_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        != 0
}

/// 拉起 embedded runtime：清扫本身走 existing-only，必须先有活实例（生产里由会话触发）。
async fn start_runtime(ingest: &Arc<Ingest>, workspace: &str) {
    ingest
        .base
        .call(
            &ingest.target_json(),
            RuntimePolicy::StartIfNeeded,
            "session/list",
            json!({
                "sessionIds": ["child-stale"],
                "includeArchived": true,
                "workspace": {"workspacePath": workspace, "workspaceKey": workspace},
            }),
        )
        .await
        .expect("embedded runtime answers session/list");
}

#[tokio::test(flavor = "multi_thread")]
async fn repair_subagent_rows_soft_deletes_only_cli_confirmed_children() {
    let (_dir, ingest, mut events, workspace) = ingest_with_legacy_child_rows().await;
    start_runtime(&ingest, &workspace).await;

    // 首帧可见集合只含 child-visible；child-stale 是旧版本遗留的主列表行。
    let visible: std::collections::BTreeSet<String> = ["child-visible".to_owned()].into();
    crate::task_index_syncer::repair::repair_subagent_rows(&ingest, &visible).await;

    assert!(
        deleted_flag(&ingest.read.path, "child-stale"),
        "旧 child 行应被软删除"
    );
    assert!(
        !deleted_flag(&ingest.read.path, "child-visible"),
        "首帧可见的会话不是候选"
    );
    assert!(
        !deleted_flag(&ingest.read.path, "interactive-stale"),
        "CLI 确认不是 subagent_child 的行不能删除"
    );
    let event = next_event(&mut events, "task_meta_changed").await;
    assert_eq!(event["workspacePath"], workspace);
    assert!(
        event.get("taskId").is_none(),
        "TS 同形：无 taskMeta 的列表变更事件不带 taskId"
    );

    // 幂等：再次清扫没有剩余候选，也不重复广播。
    crate::task_index_syncer::repair::repair_subagent_rows(&ingest, &visible).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(120), events.next())
            .await
            .is_err(),
        "重复清扫必须是 no-op"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn first_snapshot_dispatches_repair_and_broadcasts_list_change() {
    let (_dir, ingest, mut events, workspace) = ingest_with_legacy_child_rows().await;
    start_runtime(&ingest, &workspace).await;
    sync_topic::set_subscription_for_tests(&ingest, TopicKind::SessionsIndex, "sub-1", "e1");
    // 夹具的 topic 固定为 `/w`；本用例的 workspace 是临时目录，帧必须带真实 topic 才会入队。
    let topic = format!("sessions-index/{workspace}");
    let mut wire = index_wire(
        "sub-1",
        0,
        1,
        "initial",
        snapshot_payload(vec![summary("child-visible", "running", "首条输入")]),
    );
    wire["topic"] = json!(topic);
    wire["frame"]["topic"] = json!(topic);
    sync_topic::handle_wire(&ingest, TopicKind::SessionsIndex, &wire);
    // 首帧之后由 syncer 自己触发清扫（不是测试直接调用 repair）。
    wait_for(
        || deleted_flag(&ingest.read.path, "child-stale").then_some(()),
        "first-snapshot subagent sweep",
    )
    .await;
    assert!(!deleted_flag(&ingest.read.path, "child-visible"));
    assert!(!deleted_flag(&ingest.read.path, "interactive-stale"));
    let event = next_event(&mut events, "task_meta_changed").await;
    assert_eq!(event["workspacePath"], workspace);
}
