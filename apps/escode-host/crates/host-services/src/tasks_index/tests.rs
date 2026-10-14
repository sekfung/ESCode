use super::*;

/// 夹具：按生成资产建一个空库（表/索引/迁移账本齐全），用于打开与对账测试。
fn fixture(ledger: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    let connection = Connection::open(&path).unwrap();
    for statement in schema::MIGRATED_DDL {
        connection.execute_batch(statement).unwrap();
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks_schema_migration (
               id TEXT PRIMARY KEY, checksum TEXT NOT NULL, time_applied INTEGER NOT NULL)",
        )
        .unwrap();
    for (id, checksum) in ledger {
        connection
            .execute(
                "INSERT INTO tasks_schema_migration (id, checksum, time_applied) VALUES (?1, ?2, 0)",
                rusqlite::params![id, checksum],
            )
            .unwrap();
    }
    dir
}

fn expected_ledger() -> Vec<(&'static str, &'static str)> {
    schema::EXPECTED_MIGRATIONS.to_vec()
}

#[test]
fn generated_assets_are_self_consistent() {
    verify_assets().expect("生成资产自校验");
    assert_eq!(schema::SCHEMA_VERSION, 1);
    assert_eq!(schema::EXPECTED_MIGRATIONS.len(), 3, "三个迁移");
    assert!(
        schema::MIGRATED_DDL
            .iter()
            .any(|ddl| ddl.contains("CREATE TABLE tasks (")),
        "DDL 含 tasks 表"
    );
}

#[test]
fn open_accepts_a_ledger_that_matches_the_generated_asset() {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect("open");
    assert_eq!(index.migration_ledger().unwrap().len(), 3);
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
            assert_eq!(missing, vec!["0003_official_glm_selection".to_owned()]);
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
