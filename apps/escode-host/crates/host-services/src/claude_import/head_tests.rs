use super::*;
use serde_json::json;

#[test]
fn sidechain_marker_is_detected_at_any_level() {
    assert!(has_sidechain_marker(&[
        json!({"type":"user","isSidechain":true})
    ]));
    assert!(has_sidechain_marker(&[
        json!({"type":"user","message":{"isSidechain":true}})
    ]));
    assert!(!has_sidechain_marker(&[
        json!({"type":"user","message":{"content":"hi"}})
    ]));
}

#[test]
fn head_info_uses_the_first_visible_user_text() {
    let entries = vec![
        json!({"type":"user","isMeta":true,"cwd":"/meta","message":{"content":"meta"}}),
        json!({
            "type":"user",
            "cwd":"/repo",
            "timestamp": 1_700_000_000_000_i64,
            "message":{"content":[{"type":"text","text":"<ide_opened_file>x</ide_opened_file> 帮我评审"}]},
        }),
        json!({"type":"user","cwd":"/other","message":{"content":"second"}}),
    ];
    let info = extract_head_info(&entries);
    assert_eq!(info.workspace_path.as_deref(), Some("/repo"));
    assert_eq!(info.preview_title.as_deref(), Some("帮我评审"));
    assert_eq!(info.created_at, Some(1_700_000_000_000));
}

#[test]
fn sanitize_strips_command_blocks() {
    assert_eq!(
        sanitize_visible_text("a <local-command-stdout>x</local-command-stdout> b"),
        "a   b"
    );
}
