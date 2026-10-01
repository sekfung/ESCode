//! plugin_marketplace.rs 的单测（从内联 `mod tests` 挪出，保持文件在 400 行以内）。
use super::*;

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
