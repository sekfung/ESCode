use super::bootstrap::ensure_project_schema;
use super::{TasksIndex, TasksIndexError, fixture, schema};
use rusqlite::Connection;

fn tasks_only_db(path: &std::path::Path) {
    let connection = Connection::open(path).unwrap();
    fixture::apply_tasks_schema(&connection).unwrap();
}

fn ledger(path: &std::path::Path) -> Vec<(String, String)> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT id, checksum FROM schema_migration ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

#[test]
fn bootstraps_project_schema_for_an_empty_tasks_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    tasks_only_db(&path);

    ensure_project_schema(&path).unwrap();

    let expected: Vec<(String, String)> = schema::EXPECTED_PROJECT_MIGRATIONS
        .iter()
        .map(|(id, checksum)| ((*id).to_owned(), (*checksum).to_owned()))
        .collect();
    assert_eq!(ledger(&path), expected);
    assert!(TasksIndex::open(&path).is_ok());
    let connection = Connection::open(&path).unwrap();
    assert!(
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='projects')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
}

#[test]
fn leaves_an_existing_project_ledger_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    let connection = Connection::open(&path).unwrap();
    fixture::apply_generated_schema(&connection, schema::EXPECTED_MIGRATIONS).unwrap();
    drop(connection);
    let before = ledger(&path);

    ensure_project_schema(&path).unwrap();

    assert_eq!(ledger(&path), before);
    assert!(TasksIndex::open(&path).is_ok());
}

#[test]
fn refuses_a_non_empty_tasks_index_without_a_project_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    tasks_only_db(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO tasks (workspace_key, workspace_path, task_id, created_at, updated_at)
             VALUES ('ws', '/tmp/ws', 'task-1', 1, 1)",
            [],
        )
        .unwrap();
    drop(connection);

    let error = ensure_project_schema(&path).unwrap_err();
    assert!(matches!(error, TasksIndexError::MigrationMismatch { .. }));
    assert!(
        !Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migration')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
}

#[test]
fn refuses_partial_project_tables_without_a_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    tasks_only_db(&path);
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE hosts (host_id TEXT PRIMARY KEY, kind TEXT NOT NULL)")
        .unwrap();

    let error = ensure_project_schema(&path).unwrap_err();
    assert!(matches!(error, TasksIndexError::MigrationMismatch { .. }));
}

#[test]
fn refuses_when_the_tasks_ledger_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tasks-index.sqlite");
    drop(Connection::open(&path).unwrap());

    let error = ensure_project_schema(&path).unwrap_err();
    assert!(matches!(error, TasksIndexError::MigrationMismatch { .. }));
}
