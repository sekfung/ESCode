use super::test_support::{expected_ledger, fixture};
use super::*;

#[test]
fn generated_assets_are_self_consistent() {
    verify_assets().expect("生成资产自校验");
    assert_eq!(schema::SCHEMA_VERSION, 2);
    assert_eq!(schema::EXPECTED_MIGRATIONS.len(), 6, "六个 tasks 迁移");
    assert_eq!(
        schema::EXPECTED_PROJECT_MIGRATIONS.len(),
        3,
        "三个 project 迁移"
    );
    assert!(
        schema::MIGRATED_DDL
            .iter()
            .any(|ddl| ddl.contains("CREATE TABLE tasks (")),
        "DDL 含 tasks 表"
    );
    assert!(
        schema::MIGRATED_DDL
            .iter()
            .any(|ddl| ddl.contains("CREATE TABLE projects (")),
        "DDL 含 projects 表"
    );
    assert!(
        schema::MIGRATED_DDL
            .iter()
            .any(|ddl| ddl.contains("CREATE TABLE task_group_admissions (")),
        "DDL 含 task_group_admissions 表"
    );
}

#[test]
fn open_accepts_a_ledger_that_matches_the_generated_asset() {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect("open");
    assert_eq!(index.migration_ledger().unwrap().len(), 6);
    assert_eq!(index.project_migration_ledger().unwrap().len(), 3);
}

#[test]
fn open_uses_the_long_lived_repo_busy_timeout() {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect("open");
    let timeout: i64 = index
        .connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("busy_timeout");
    assert_eq!(timeout, 5000, "与 TS 长连接 TaskIndexRepo 同口径");
}

#[test]
fn open_refuses_a_missing_database_without_creating_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    let error = TasksIndex::open(&path).expect_err("missing");
    assert!(
        matches!(error, TasksIndexError::Missing { .. }),
        "{error:?}"
    );
    assert!(!path.exists(), "Host 不自行建库");
}

#[test]
fn open_refuses_a_ledger_with_missing_unknown_or_mismatched_entries() {
    // 缺一条迁移。
    let dir = fixture(&expected_ledger()[..2]);
    let error =
        TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect_err("missing entry");
    match error {
        TasksIndexError::MigrationMismatch { missing, .. } => {
            assert_eq!(
                missing,
                vec![
                    "0003_official_glm_selection".to_owned(),
                    "0004_drop_off_peak".to_owned(),
                    "0005_project_execution_scopes".to_owned(),
                    "0006_task_group_admissions".to_owned()
                ]
            );
        }
        other => panic!("{other:?}"),
    }

    // 多出一条未知迁移（TS 更新版写过）。
    let mut ledger = expected_ledger();
    ledger.push(("0004_future", "deadbeef"));
    let dir = fixture(&ledger);
    let error = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect_err("unknown");
    match error {
        TasksIndexError::MigrationMismatch { unknown, .. } => {
            assert_eq!(unknown, vec!["0004_future".to_owned()]);
        }
        other => panic!("{other:?}"),
    }

    // checksum 不符（同一 id 的迁移内容被改过）。
    let mut ledger = expected_ledger();
    ledger[0].1 = "0000000000000000000000000000000000000000000000000000000000000000";
    let dir = fixture(&ledger);
    let error = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect_err("mismatch");
    match error {
        TasksIndexError::MigrationMismatch { mismatched, .. } => {
            assert_eq!(mismatched, vec!["0001_adopt_task_schema".to_owned()]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn get_task_meta_returns_none_for_missing_and_deleted_rows() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    let index = TasksIndex::open(&path).expect("open");
    assert!(
        index
            .get_task_meta("/w", None, "task_missing")
            .unwrap()
            .is_none()
    );

    insert_task_row(&path, "task_deleted", 1);
    assert!(
        index
            .get_task_meta("/w", None, "task_deleted")
            .unwrap()
            .is_none(),
        "deleted=1 不返回"
    );
}

#[test]
fn get_task_meta_uses_the_workspace_key_identity_rule() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    let index = TasksIndex::open(&path).expect("open");
    insert_task_row(&path, "task_1", 0);
    // workspace_key = identity.trim() || path：按身份写入的行用路径查不到。
    assert!(
        index
            .get_task_meta("/w", Some("remote:ssh:dev"), "task_1")
            .unwrap()
            .is_none()
    );
    assert!(index.get_task_meta("/w", None, "task_1").unwrap().is_some());
}

/// 插入一行最小可用 tasks 记录（meta_json 合法）。
fn insert_task_row(path: &std::path::Path, task_id: &str, deleted: i64) {
    let connection = Connection::open(path).unwrap();
    let meta = serde_json::json!({
        "taskId": task_id,
        "traceId": format!("escode-{task_id}"),
        "title": "title",
        "workspacePath": "/w",
        "createdAt": 1,
        "updatedAt": 2,
        "mode": "build",
    });
    connection
        .execute(
            "INSERT INTO tasks (
               workspace_key, workspace_path, workspace_identity, task_id, title, mode,
               created_at, updated_at, deleted, meta_json
             ) VALUES ('/w', '/w', NULL, ?1, 'title', 'build', 1, 2, ?2, ?3)",
            rusqlite::params![task_id, deleted, meta.to_string()],
        )
        .unwrap();
}

/// 写入搜索/分组相关的额外夹具（全文文本、组、成员、顶层排序）。
fn seed_query_fixtures(path: &std::path::Path) {
    let connection = Connection::open(path).unwrap();
    let rows = [
        ("task_s1", "glm", 0, 0, 0, 300, "部署脚本与回滚说明"),
        ("task_s2", "glm", 0, 0, 0, 200, "评审 Rust Host 的迁移"),
        ("task_s3", "glm", 1, 0, 0, 100, "置顶：迁移检查清单"),
        ("task_s4", "glm", 0, 1, 0, 50, "已归档内容"),
    ];
    for (task_id, provider, pinned, archived, deleted, updated_at, searchable) in rows {
        let meta = serde_json::json!({
            "taskId": task_id,
            "traceId": format!("escode-{task_id}"),
            "title": format!("标题 {task_id}"),
            "workspacePath": "/w",
            "createdAt": 1,
            "updatedAt": updated_at,
            "mode": "build",
        });
        connection
            .execute(
                "INSERT INTO tasks (workspace_key, workspace_path, task_id, title, mode, provider,
                   created_at, updated_at, pinned, archived, deleted, searchable_text, meta_json)
                 VALUES ('/w', '/w', ?1, ?2, 'build', ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    task_id,
                    format!("标题 {task_id}"),
                    provider,
                    updated_at,
                    pinned,
                    archived,
                    deleted,
                    searchable,
                    meta.to_string()
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO task_groups (group_id, title, color, created_at, updated_at)
             VALUES ('group_1', '分组一', 'blue', 1, 2)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO task_group_members
               (group_id, workspace_key, workspace_path, workspace_identity, task_id, sort_order,
                added_at, created_at, updated_at)
             VALUES ('group_1', '/w', '/w', NULL, 'task_s1', 5, 10, 10, 10)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
             VALUES ('group', 'group_1', 1, 1, 1), ('task', '[\"/w\",\"task_s2\"]', 2, 1, 1)",
            [],
        )
        .unwrap();
}

#[test]
fn query_task_list_filters_sorts_and_paginates() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    seed_query_fixtures(&path);
    let index = TasksIndex::open(&path).unwrap();
    let scopes = [query::WorkspaceScope {
        workspace_path: "/w",
        workspace_identity: None,
        workspace_purpose: Some("project"),
    }];

    // timeline：pinned=0 且 archived=0，按 updated_at DESC。
    let result = index
        .query_task_list(&query::TaskListQuery {
            kind: "timeline",
            workspace_scopes: &scopes,
            sort_by: "updated",
            search: None,
            limit: None,
            provider: Some("glm"),
        })
        .unwrap();
    assert_eq!(result["total"], 2, "{result}");
    assert_eq!(result["hasMore"], false);
    let ids: Vec<&str> = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["taskId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["task_s1", "task_s2"]);
    assert_eq!(result["items"][0]["workspacePurpose"], "project");

    // 搜索命中正文：附 searchSnippet/searchSnippets。
    let searched = index
        .query_task_list(&query::TaskListQuery {
            kind: "timeline",
            workspace_scopes: &scopes,
            sort_by: "updated",
            search: Some("迁移"),
            limit: None,
            provider: Some("glm"),
        })
        .unwrap();
    assert_eq!(searched["total"], 1, "{searched}");
    let item = &searched["items"][0];
    assert_eq!(item["taskId"], "task_s2");
    assert!(
        item["searchSnippet"].as_str().unwrap().contains("迁移"),
        "{item}"
    );
    assert!(!item["searchSnippets"].as_array().unwrap().is_empty());

    // limit：total 仍为命中总数，hasMore 为 true。
    let limited = index
        .query_task_list(&query::TaskListQuery {
            kind: "timeline",
            workspace_scopes: &scopes,
            sort_by: "created",
            search: None,
            limit: Some(1),
            provider: Some("glm"),
        })
        .unwrap();
    assert_eq!(limited["total"], 2);
    assert_eq!(limited["hasMore"], true);
    assert_eq!(limited["items"].as_array().unwrap().len(), 1);

    // 空 scopes：空结果（不查库）。
    let empty = index
        .query_task_list(&query::TaskListQuery {
            kind: "timeline",
            workspace_scopes: &[],
            sort_by: "updated",
            search: None,
            limit: None,
            provider: Some("glm"),
        })
        .unwrap();
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["hasMore"], false);

    // pinned / archived 分区。
    for (kind, expected) in [("pinned", vec!["task_s3"]), ("archived", vec!["task_s4"])] {
        let result = index
            .query_task_list(&query::TaskListQuery {
                kind,
                workspace_scopes: &scopes,
                sort_by: "updated",
                search: None,
                limit: None,
                provider: Some("glm"),
            })
            .unwrap();
        let ids: Vec<&str> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["taskId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, expected, "{kind}");
    }
}

#[test]
fn query_grouped_structure_returns_groups_members_and_orders() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    seed_query_fixtures(&path);
    let index = TasksIndex::open(&path).unwrap();

    let structure = index
        .query_grouped_task_view_structure(&[("/w", None)])
        .unwrap();
    assert_eq!(structure["groups"].as_array().unwrap().len(), 1);
    assert_eq!(structure["groups"][0]["id"], "group_1");
    assert_eq!(structure["groups"][0]["color"], "blue");
    let member = &structure["members"][0];
    assert_eq!(member["groupId"], "group_1");
    assert_eq!(member["taskId"], "task_s1");
    assert_eq!(member["sortOrder"], 5);
    assert!(member.get("workspaceIdentity").is_none(), "{member}");
    let orders = structure["topLevelOrders"].as_array().unwrap();
    assert_eq!(orders.len(), 2);
    assert_eq!(orders[0]["type"], "group");
    assert_eq!(orders[0]["groupId"], "group_1");
    assert_eq!(orders[1]["type"], "task");
    assert_eq!(orders[1]["taskId"], "task_s2");
    assert_eq!(orders[1]["workspaceKey"], "/w");

    // 不可见 workspace 的 scope：bootstrap 组被过滤，但成员仍全量返回。
    let other = index
        .query_grouped_task_view_structure(&[("/other", None)])
        .unwrap();
    assert_eq!(
        other["groups"].as_array().unwrap().len(),
        1,
        "非 bootstrap 组不受 scope 过滤"
    );
    assert_eq!(other["members"].as_array().unwrap().len(), 1);
}

#[test]
fn open_rejects_a_project_ledger_mismatch() {
    let dir = fixture(&expected_ledger());
    let path = dir.path().join("tasks-index.sqlite");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE schema_migration SET checksum='corrupt' WHERE id='0005_projects'",
                [],
            )
            .unwrap();
    }
    let error = TasksIndex::open(&path).expect_err("project ledger mismatch");
    assert!(matches!(error, TasksIndexError::MigrationMismatch { .. }));
}
