//! `gen-ui` schema 校验（`rust-u2-gen-ui.md` §4/§6）：路径分支、strict 键、缺省 null、16 KiB 边界。
use super::*;

#[test]
fn document_path_accepts_only_absolute_lowercase_hyphenated_html() {
    for valid in [
        "/root/visualizations/abc/report.html",
        "/root/visualizations/abc/my-chart-2.html",
        "/root/visualizations/a.html",
        "C:\\Users\\me\\visualizations\\x\\a-b.html",
        "\\\\server\\share\\a.html",
    ] {
        assert!(is_valid_document_path(valid), "应接受: {valid}");
    }
    let long = format!("/root/{}", "a".repeat(4200));
    for (label, invalid) in [
        ("相对路径", "visualizations/a.html"),
        ("`..` 段", "/root/visualizations/../a.html"),
        ("大写末段", "/root/visualizations/A.html"),
        ("下划线末段", "/root/visualizations/a_b.html"),
        ("非 .html", "/root/visualizations/a.htm"),
        ("空末段", "/root/visualizations/.html"),
        ("含 NUL", "/root/visualizations/a\0.html"),
        ("含换行", "/root/visualizations/a\n.html"),
        ("超长", long.as_str()),
        ("空串", ""),
    ] {
        assert!(
            !is_valid_document_path(invalid),
            "应拒绝({label}): {invalid}"
        );
    }
}

#[test]
fn scope_and_target_reject_unknown_keys_and_null_optionals() {
    let scope = serde_json::json!({"workspacePath": "/w", "sessionId": "s1"});
    assert_eq!(
        parse_scope(&scope).unwrap(),
        Scope {
            workspace_path: "/w".to_owned(),
            workspace_identity: None,
            session_id: "s1".to_owned(),
        }
    );
    // strict：多一个键即失败。
    let extra = serde_json::json!({"workspacePath": "/w", "sessionId": "s1", "extra": 1});
    assert!(parse_scope(&extra).is_err());
    // optional 字段不接受 null（zod `.optional()` 的口径）。
    let null_identity =
        serde_json::json!({"workspacePath": "/w", "workspaceIdentity": null, "sessionId": "s1"});
    assert!(parse_scope(&null_identity).is_err());
    // 空串不满足 min(1)。
    assert!(parse_scope(&serde_json::json!({"workspacePath": "", "sessionId": "s1"})).is_err());

    // target 允许 path 键，但不允许多余键；path 必须过路径 schema。
    let target = serde_json::json!({"workspacePath": "/w", "sessionId": "s1", "path": "/r/a.html"});
    assert_eq!(parse_target(&target).unwrap().path, "/r/a.html");
    assert!(
        parse_target(&serde_json::json!({
            "workspacePath": "/w", "sessionId": "s1", "path": "a.html"
        }))
        .is_err()
    );
    assert!(
        parse_target(&serde_json::json!({
            "workspacePath": "/w", "sessionId": "s1", "path": "/r/a.html", "extra": 1
        }))
        .is_err()
    );
}

#[test]
fn widget_state_defaults_to_null_and_caps_at_16_kib() {
    assert_eq!(
        parse_widget_state(&serde_json::json!({})).unwrap(),
        serde_json::json!({"modelContent": null, "privateContent": null})
    );
    assert_eq!(
        parse_widget_state(&serde_json::json!({"modelContent": {"a": 1}})).unwrap(),
        serde_json::json!({"modelContent": {"a": 1}, "privateContent": null})
    );
    assert!(parse_widget_state(&serde_json::json!({"other": 1})).is_err());
    assert!(parse_widget_state(&serde_json::json!("text")).is_err());

    // 16 KiB 按**序列化后的 UTF-8 字节**：骨架 `{"modelContent":"","privateContent":null}` 占 41 字节。
    let filler = |bytes: usize| "x".repeat(bytes);
    let fits = serde_json::json!({"modelContent": filler(16 * 1024 - 41)});
    assert_eq!(
        serde_json::to_string(&parse_widget_state(&fits).unwrap())
            .unwrap()
            .len(),
        16 * 1024,
        "恰好 16 KiB 应允许"
    );
    let over = serde_json::json!({"modelContent": filler(16 * 1024 - 40)});
    assert!(
        parse_widget_state(&over).unwrap_err().contains("16 KiB"),
        "超过 16 KiB 应拒绝"
    );
}

#[test]
fn scope_key_matches_the_js_array_serialization() {
    let scope = |identity: Option<&str>| Scope {
        workspace_path: "/w".to_owned(),
        workspace_identity: identity.map(str::to_owned),
        session_id: "s1".to_owned(),
    };
    assert_eq!(scope_key(&scope(None)), r#"["/w","s1"]"#);
    assert_eq!(scope_key(&scope(Some("ssh:host"))), r#"["ssh:host","s1"]"#);
    // 有 trim；全空白 identity 回落路径（`identity?.trim() || path`）。
    assert_eq!(
        scope_key(&scope(Some("  ssh:host  "))),
        r#"["ssh:host","s1"]"#
    );
    assert_eq!(scope_key(&scope(Some("   "))), r#"["/w","s1"]"#);
    // 非 ASCII 保持字面（JS 的 `JSON.stringify` 不转义非 ASCII）。
    let unicode = Scope {
        workspace_path: "/工作区".to_owned(),
        workspace_identity: None,
        session_id: "会话".to_owned(),
    };
    assert_eq!(scope_key(&unicode), "[\"/工作区\",\"会话\"]");
}
