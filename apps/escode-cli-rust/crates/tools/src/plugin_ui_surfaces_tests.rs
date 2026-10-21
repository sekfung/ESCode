//! `plugins/listUiSurfaces` 清单解析单测（TS `parsePluginUiSurfaces` 对照语料）。
use super::*;
use serde_json::json;

fn declared() -> Vec<String> {
    vec!["widget".to_owned()]
}

#[test]
fn parses_a_valid_surface_and_namespaces_the_server() {
    let raw = json!({
        "id": "board.view",
        "title": "  Board  ",
        "icon": "  icon.svg  ",
        "server": " widget ",
        "resourceUri": "ui://board/index.html",
        "availability": "session"
    });
    let surface = parse_surface(&raw, &declared(), "my-plugin").expect("surface");
    assert_eq!(surface["id"], "board.view");
    assert_eq!(surface["title"], "Board");
    assert_eq!(surface["icon"], "icon.svg");
    assert_eq!(surface["server"], "plugin:my-plugin:widget");
    assert_eq!(surface["resourceUri"], "ui://board/index.html");
    assert_eq!(surface["availability"], "session");
}

#[test]
fn accepts_a_locale_title_map_and_drops_empty_entries() {
    let raw = json!({
        "id": "board",
        "title": {"en": "Board", "zh": "   ", "ja": "ボード"},
        "server": "widget",
        "resourceUri": "ui://board/index.html",
        "availability": "session"
    });
    let surface = parse_surface(&raw, &declared(), "p").expect("surface");
    assert_eq!(surface["title"], json!({"en": "Board", "ja": "ボード"}));
}

#[test]
fn rejects_invalid_id_server_uri_and_availability() {
    let base = |id: &str, server: &str, uri: &str, availability: &str| json!({"id": id, "title": "T", "server": server, "resourceUri": uri, "availability": availability});
    assert!(
        parse_surface(
            &base("-bad", "widget", "ui://x", "session"),
            &declared(),
            "p"
        )
        .is_none()
    );
    assert!(parse_surface(&base("ok", "other", "ui://x", "session"), &declared(), "p").is_none());
    assert!(
        parse_surface(
            &base("ok", "widget", "https://x", "session"),
            &declared(),
            "p"
        )
        .is_none()
    );
    assert!(parse_surface(&base("ok", "widget", "ui://x", "always"), &declared(), "p").is_none());
    assert!(
        parse_surface(
            &base(&"a".repeat(129), "widget", "ui://x", "session"),
            &declared(),
            "p"
        )
        .is_none()
    );
    assert!(parse_surface(&base("ok", "widget", "ui://x", "session"), &declared(), "p").is_some());
}
