use super::test_support::fixture;
use super::*;
use serde_json::json;

fn index() -> (tempfile::TempDir, TasksIndex) {
    let dir = fixture(&super::test_support::expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).expect("open");
    (dir, index)
}

fn target() -> serde_json::Value {
    json!({"kind":"ssh","host":"example.test","port":22,"username":"root"})
}

#[test]
fn saves_and_reads_remote_host_entries() {
    let (_dir, index) = index();
    let host = index
        .remote_project_save_host(&json!({"id":"remote:server:s1","target":target()}))
        .expect("save host");
    assert_eq!(host["id"], "remote:server:s1");
    assert_eq!(host["kind"], "ssh");
    assert!(
        host["membershipId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );

    let directory = index.remote_project_read_directory().unwrap();
    assert_eq!(directory["hosts"].as_array().unwrap().len(), 1);
    assert!(directory["entries"].as_array().unwrap().is_empty());

    let entry = index
        .remote_project_add_entry(&json!({
            "hostId":"remote:server:s1",
            "project": {"projectId":"p1","name":"Demo","roots":[{"rootPath":"/srv/demo","isPrimary":true}]}
        }))
        .expect("add entry");
    assert_eq!(entry["projectId"], "p1");
    assert_eq!(entry["roots"][0]["rootPath"], "/srv/demo");

    let directory = index.remote_project_read_directory().unwrap();
    assert_eq!(directory["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn add_entry_is_idempotent_and_refresh_uses_cas() {
    let (_dir, index) = index();
    index
        .remote_project_save_host(&json!({"id":"remote:server:s1","target":target()}))
        .unwrap();
    let first = index
        .remote_project_add_entry(&json!({
            "hostId":"remote:server:s1",
            "project": {"projectId":"p1","name":"Demo","roots":["/srv/demo"]}
        }))
        .unwrap();
    let again = index
        .remote_project_add_entry(&json!({
            "hostId":"remote:server:s1",
            "project": {"projectId":"p1","name":"Demo 2","roots":["/srv/demo"]}
        }))
        .unwrap();
    assert_eq!(first["id"], again["id"]);
    assert_eq!(again["name"], "Demo 2");

    let stale = index.remote_project_refresh_entry(&json!({
        "entryId": first["id"],
        "expectedLastSyncedAt": 0,
        "project": {"projectId":"p1","name":"Stale","roots":["/srv/demo"]}
    }));
    assert!(!stale.unwrap(), "stale version must not apply");

    let applied = index.remote_project_refresh_entry(&json!({
        "entryId": first["id"],
        "expectedLastSyncedAt": again["lastSyncedAt"],
        "project": {"projectId":"p1","name":"Fresh","roots":["/srv/demo"]}
    }));
    assert!(applied.unwrap());
    let directory = index.remote_project_read_directory().unwrap();
    assert_eq!(directory["entries"][0]["name"], "Fresh");
}

#[test]
fn removing_host_cascades_entries() {
    let (_dir, index) = index();
    index
        .remote_project_save_host(&json!({"id":"remote:server:s1","target":target()}))
        .unwrap();
    index
        .remote_project_add_entry(&json!({
            "hostId":"remote:server:s1",
            "project": {"projectId":"p1","name":"Demo","roots":["/srv/demo"]}
        }))
        .unwrap();
    assert!(
        index
            .remote_project_remove_host("remote:server:s1")
            .unwrap()
    );
    let directory = index.remote_project_read_directory().unwrap();
    assert!(directory["hosts"].as_array().unwrap().is_empty());
    assert!(directory["entries"].as_array().unwrap().is_empty());
}

#[test]
fn local_host_is_rejected() {
    let (_dir, index) = index();
    let error = index
        .remote_project_save_host(&json!({"id":"local","target":target()}))
        .expect_err("local host must be rejected");
    assert!(error.to_string().contains("remote-project.local-host"));
}
