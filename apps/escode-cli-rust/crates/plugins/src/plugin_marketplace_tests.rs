//! plugin_marketplace.rs 的单测（从内联 `mod tests` 挪出，保持文件在 400 行以内）。
use super::*;
use serde_json::json;

#[test]
fn coerce_matches_node_semver() {
    assert_eq!(coerce("1.2.3"), Some((1, 2, 3)));
    assert_eq!(coerce("v2"), Some((2, 0, 0)));
    assert_eq!(coerce("release-1.4-beta"), Some((1, 4, 0)));
    assert_eq!(coerce("1.2.3.4"), Some((1, 2, 3)));
    assert_eq!(coerce("abc"), None);
}

#[test]
fn update_status_follows_ts_axes() {
    assert_eq!(
        update_status(Some("1.0.0"), None, Some("1.1.0"), None),
        "update-available"
    );
    assert_eq!(
        update_status(Some("1.1.0"), None, Some("1.0.0"), None),
        "none"
    );
    assert_eq!(
        update_status(Some("abc"), None, Some("abd"), None),
        "version-changed"
    );
    assert_eq!(update_status(Some("abc"), None, Some("abc"), None), "none");
    assert_eq!(update_status(None, None, Some("1.0.0"), None), "none");
    assert_eq!(
        update_status(None, None, None, Some("x")),
        "version-changed"
    );
    assert_eq!(update_status(None, Some("x"), None, Some("x")), "none");
    assert_eq!(
        update_status(None, Some("y"), None, Some("x")),
        "update-available"
    );
    assert_eq!(update_status(Some("1.0.0"), None, None, None), "none");
}

#[test]
fn marketplace_name_pattern() {
    assert!(valid_marketplace_name("zcode-plugins-official"));
    assert!(valid_marketplace_name("a.b_c-1"));
    assert!(!valid_marketplace_name("Upper"));
    assert!(!valid_marketplace_name("-lead"));
    assert!(!valid_marketplace_name(""));
}

#[test]
fn listing_parses_store_fields() {
    let entry = json!({
        "name": "p",
        "displayName": "P",
        "displayName_i18n": {"zh": "批", "bad": 1},
        "icon": " ",
        "category": "dev",
        "author": {"name": " A ", "url": "https://a"},
        "examplePrompts": ["x", " ", 1],
        "examplePrompts_i18n": {"zh": ["甲", 2], "en": []},
        "requiresPaidPlan": "true",
    });
    assert_eq!(
        listing(entry.as_object().unwrap()),
        Some(json!({
            "displayName": "P",
            "displayNameI18n": {"zh": "批"},
            "category": "dev",
            "author": "A",
            "authorUrl": "https://a",
            "examplePrompts": ["x"],
            "examplePromptsI18n": {"zh": ["甲"]},
        }))
    );
    assert_eq!(listing(json!({"name": "p"}).as_object().unwrap()), None);
}

#[test]
fn recovers_state_files_and_marketplace_directory_like_node() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(storage.join("marketplaces")).unwrap();

    let known = storage.join("known_marketplaces.json");
    std::fs::write(
        &known,
        r#"{"version":1,"marketplaces":[{"id":"recover-market","name":"Recover","pluginCount":1,"source":{"source":"url","url":"https://example.com/marketplace.json"}}]}"#,
    )
    .unwrap();
    std::fs::rename(&known, storage.join(".known_marketplaces.json.backup")).unwrap();

    let installed = storage.join("installed_plugins.json");
    std::fs::write(
        &installed,
        r#"{"version":1,"plugins":[{"id":"recover-plugin@recover-market","name":"recover-plugin","marketplace":"recover-market","version":"1.0.0","installPath":"/tmp/recover-plugin","installedAt":"2026-01-01T00:00:00.000Z","scope":"user"}]}"#,
    )
    .unwrap();
    std::fs::rename(&installed, storage.join(".installed_plugins.json.backup")).unwrap();

    let marketplace = storage.join("marketplaces").join("recover-market");
    std::fs::create_dir_all(&marketplace).unwrap();
    std::fs::write(
        marketplace.join("marketplace.json"),
        r#"{"name":"recover-market","plugins":[{"name":"recover-plugin","source":"./recover-plugin"}]}"#,
    )
    .unwrap();
    std::fs::rename(
        &marketplace,
        storage.join("marketplaces").join(".recover-market.backup"),
    )
    .unwrap();

    assert_eq!(known_marketplaces(&storage)[0]["id"], "recover-market");
    assert_eq!(
        installed_records(&storage)[0]["id"],
        "recover-plugin@recover-market"
    );
    assert_eq!(
        manifest(&storage, "recover-market").unwrap().entries[0].name,
        "recover-plugin"
    );
    assert!(known.exists());
    assert!(installed.exists());
    assert!(marketplace.exists());
}
