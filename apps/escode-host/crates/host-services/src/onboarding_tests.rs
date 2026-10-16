use serde_json::json;

use super::*;

fn entry(occupation: Option<&str>) -> Value {
    json!({"occupation": occupation, "interfaceMode": "coding", "memoryEnabled": true,
        "proactiveSuggestionsEnabled": null, "completedAt": "2026-01-01T00:00:00.000Z", "extra": 1})
}

fn records(dir: &Path) -> OnboardingRecords {
    OnboardingRecords::new(OnboardingRecords::path(dir))
}

#[test]
fn iso_formatting_matches_to_iso_string() {
    assert_eq!(iso_from_millis(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        iso_from_millis(1_760_000_000_123),
        "2025-10-09T08:53:20.123Z"
    );
    assert_eq!(
        iso_from_millis(951_782_400_000),
        "2000-02-29T00:00:00.000Z",
        "闰日"
    );
    assert_eq!(iso_from_millis(-1), "1969-12-31T23:59:59.999Z");
}

#[tokio::test]
async fn append_overwrites_the_anonymous_entry_clears_decisions_and_strips_unknown_keys() {
    let dir = tempfile::tempdir().expect("dir");
    let store = records(dir.path());
    store.dismiss("dev-1").await.expect("dismiss");
    assert_eq!(
        store.records().await.expect("file")["decisions"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    store
        .append_record("dev-1", &entry(Some("developer")))
        .await
        .expect("append");
    store
        .append_record("dev-2", &entry(Some("design")))
        .await
        .expect("append again");
    let file = store.records().await.expect("file");
    assert_eq!(file["deviceMid"], "dev-1", "deviceMid 以文件内值为权威");
    assert_eq!(
        file["entries"].as_array().map(Vec::len),
        Some(1),
        "每个用户至多一条"
    );
    assert_eq!(file["entries"][0]["occupation"], "design");
    assert!(file["entries"][0].get("extra").is_none());
    assert_eq!(file["decisions"], json!([]));
    assert!(
        store
            .append_record("dev-1", &json!({"interfaceMode": "x"}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn should_onboard_records_an_existing_user_decision_once() {
    let dir = tempfile::tempdir().expect("dir");
    let store = records(dir.path());
    assert!(
        store
            .should_onboard("d", async { false })
            .await
            .expect("fresh")
    );
    assert!(
        !store
            .should_onboard("d", async { true })
            .await
            .expect("existing local task")
    );
    let file = store.records().await.expect("file");
    assert_eq!(file["decisions"][0]["status"], "existing_local_user");
    assert!(
        !store
            .should_onboard("d", async { false })
            .await
            .expect("decided")
    );
    assert_eq!(
        store.records().await.expect("file")["decisions"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[tokio::test]
async fn settings_patch_falls_back_conservatively_and_preferences_update_in_place() {
    let dir = tempfile::tempdir().expect("dir");
    let store = records(dir.path());
    assert!(store.settings_patch().await.is_none());
    store
        .append_record("d", &entry(Some("not-in-the-enum")))
        .await
        .expect("append");
    let patch = store.settings_patch().await.expect("patch");
    assert_eq!(
        patch,
        json!({"onboardingOccupation": "other", "proactiveSuggestionsEnabled": false, "memoryEnabled": true})
    );
    store
        .update_preferences(&json!({"memoryEnabled": false}))
        .await
        .expect("update");
    assert_eq!(
        store.latest_entry().await.expect("entry")["memoryEnabled"],
        false
    );
    store.clear().await.expect("clear");
    store.clear().await.expect("clear again");
    assert!(store.records().await.is_none());
}

#[tokio::test]
async fn v1_files_load_as_v2_and_corrupt_files_read_as_missing() {
    let dir = tempfile::tempdir().expect("dir");
    let path = OnboardingRecords::path(dir.path());
    tokio::fs::create_dir_all(path.parent().expect("parent"))
        .await
        .expect("mkdir");
    tokio::fs::write(
        &path,
        json!({"version": 1, "deviceMid": "d", "entries": []}).to_string(),
    )
    .await
    .expect("write");
    let store = OnboardingRecords::new(path.clone());
    assert_eq!(store.records().await.expect("v1")["version"], 2);
    tokio::fs::write(&path, "{nope").await.expect("write");
    assert!(store.records().await.is_none());
}
