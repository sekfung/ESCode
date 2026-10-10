//! config_file.rs 的单测（从内联 `mod tests` 挪出，保持文件在 400 行以内）。
use super::*;

fn parse(text: &str) -> Json {
    Json::parse(text).unwrap()
}

#[test]
fn patch_keeps_order_and_appends_canonical_id() {
    let mut root = parse(
        r#"{"z":1,"plugins":{"dirs":["x"],"enabledPlugins":{"a@m":true,"escode-cua@escode-plugins-official":false,"b@m":false}},"y":2}"#,
    );
    patch_plugin_enabled(&mut root, "a@m", false);
    patch_plugin_enabled(&mut root, "escode-cua@escode-plugins-official", true);
    assert_eq!(
        root.compact(),
        r#"{"z":1,"plugins":{"dirs":["x"],"enabledPlugins":{"b@m":false,"a@m":false,"computer-use@escode-plugins-official":true}},"y":2}"#
    );
}

#[test]
fn patch_replaces_non_object_plugins_in_place() {
    let mut root = parse(r#"{"plugins":null,"x":1}"#);
    patch_plugin_enabled(&mut root, "p@m", true);
    assert_eq!(
        root.compact(),
        r#"{"plugins":{"enabledPlugins":{"p@m":true}},"x":1}"#
    );
}

#[test]
fn options_merge_per_key_and_clear_first() {
    let mut root = parse(
        r#"{"plugins":{"options":{"a@m":{"k1":1,"secret":"s","k2":2},"b@m":{"x":true}},"enabledPlugins":{"a@m":true}}}"#,
    );
    let input = vec![
        ("k2".to_owned(), Json::str("new")),
        ("k3".to_owned(), Json::Bool(false)),
    ];
    patch_plugin_options(&mut root, "a@m", &input, &["k1".to_owned()]);
    assert_eq!(
        root.compact(),
        r#"{"plugins":{"options":{"b@m":{"x":true},"a@m":{"secret":"s","k2":"new","k3":false}},"enabledPlugins":{"a@m":true}}}"#
    );
}

#[test]
fn reset_removes_enabled_and_options_or_only_enabled() {
    let original = r#"{"plugins":{"enabledPlugins":{"a@m":true},"options":{"a@m":{"k":1}}}}"#;
    let mut user = parse(original);
    assert!(remove_plugin(&mut user, "a@m"));
    assert_eq!(
        user.compact(),
        r#"{"plugins":{"enabledPlugins":{},"options":{}}}"#
    );
    let mut workspace = parse(original);
    assert!(remove_plugin_enabled(&mut workspace, "a@m"));
    assert_eq!(
        workspace.compact(),
        r#"{"plugins":{"enabledPlugins":{},"options":{"a@m":{"k":1}}}}"#
    );
    let mut untouched = parse(r#"{"x":1}"#);
    assert!(!remove_plugin(&mut untouched, "a@m"));
    assert!(!remove_plugin_enabled(&mut untouched, "a@m"));
    assert_eq!(untouched.compact(), r#"{"x":1}"#);
}

#[tokio::test]
async fn read_missing_is_empty_and_write_matches_json_stringify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.json");
    let mut root = read_object_or_empty(&path).await.unwrap();
    assert_eq!(root, Json::object());
    patch_plugin_enabled(&mut root, "p@m", true);
    atomic_write(&path, &root).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"plugins\": {\n    \"enabledPlugins\": {\n      \"p@m\": true\n    }\n  }\n}\n"
    );
    // 临时文件已 rename 走，目录里只剩目标文件。
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[tokio::test]
async fn read_rejects_non_object_and_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "[1]").unwrap();
    let error = read_object_or_empty(&path).await.unwrap_err().to_string();
    assert!(
        error.starts_with("Config file must contain a JSON object"),
        "{error}"
    );
    std::fs::write(&path, "{").unwrap();
    let error = read_object_or_empty(&path).await.unwrap_err().to_string();
    assert!(
        error.starts_with("Unable to parse config file as JSON"),
        "{error}"
    );
}
