//! `mcp_ui` 状态机单测（docs/specs/rust-mcp-ui.md §2/§3）。
use super::*;
use serde_json::json;

fn binding(scope_id: &str) -> Binding {
    Binding {
        workspace: "ws".into(),
        account_context: None,
        session_id: "session".into(),
        plugin_id: "plugin".into(),
        server_name: "server".into(),
        scope_id: scope_id.into(),
        resource_uri: "ui://server/index.html".into(),
        owner_web_contents_id: 7,
    }
}

fn source(generation: u64) -> Source {
    Source {
        identity: "server-key".into(),
        generation,
    }
}

fn scope() -> PluginScope {
    PluginScope {
        workspace: Some("ws".into()),
        session_id: "session".into(),
        plugin_id: "plugin".into(),
        server_name: "server".into(),
    }
}

#[test]
fn open_is_idempotent_for_the_same_logical_page_and_source() {
    let mut instances = Instances::default();
    let first = instances.open(binding("scope-1"), source(1));
    let second = instances.open(binding("scope-1"), source(1));
    assert_eq!(first, second, "同逻辑页面同连接代际必须幂等");
    let other = instances.open(binding("scope-2"), source(1));
    assert_ne!(first.generation, other.generation);
    assert_ne!(first.token, other.token);
}

#[test]
fn open_replaces_the_previous_generation_when_the_connection_changes() {
    let mut instances = Instances::default();
    let first = instances.open(binding("scope-1"), source(1));
    let second = instances.open(binding("scope-1"), source(2));
    assert!(second.generation > first.generation);
    assert!(
        instances
            .validate(&first, &scope().as_scope(), Some(&source(2)))
            .is_err()
    );
}

#[test]
fn validate_rejects_a_scope_or_connection_mismatch_and_revokes_the_lease() {
    let mut instances = Instances::default();
    let lease = instances.open(binding("scope-1"), source(1));
    let mut other = scope();
    other.plugin_id = "another-plugin".into();
    assert!(
        instances
            .validate(&lease, &other.as_scope(), Some(&source(1)))
            .is_err()
    );
    // 归属不匹配不撤销（错 server 的请求不能关闭合法页面）。
    assert!(
        instances
            .validate(&lease, &scope().as_scope(), Some(&source(1)))
            .is_ok()
    );

    assert!(
        instances
            .validate(&lease, &scope().as_scope(), Some(&source(2)))
            .is_err()
    );
    // 连接代际失配后凭证已被撤销。
    assert!(
        instances
            .validate(&lease, &scope().as_scope(), Some(&source(2)))
            .is_err()
    );
    assert!(!instances.close(&lease));
}

#[test]
fn only_if_idle_uses_the_call_pin() {
    let mut instances = Instances::default();
    let lease = instances.open(binding("scope-1"), source(1));
    assert!(!instances.is_busy(&lease));
    let pin = instances.retain(&lease, "call-1").expect("pin");
    assert!(instances.is_busy(&lease));
    instances.release(&pin);
    assert!(!instances.is_busy(&lease));
}

#[test]
fn close_session_revokes_every_instance_of_that_session() {
    let mut instances = Instances::default();
    let lease = instances.open(binding("scope-1"), source(1));
    instances.close_session("session");
    assert!(!instances.close(&lease));
    let mut later = binding("scope-2");
    later.session_id = "other".into();
    assert!(instances.open(later, source(1)).generation >= 1);
}

#[test]
fn calls_reject_duplicates_and_keep_terminal_state() {
    let mut calls = Calls::default();
    let cancel = calls.begin("session", "token", "call-1").expect("first");
    assert!(calls.begin("session", "token", "call-1").is_err());
    assert!(!cancel.is_cancelled());
    assert!(calls.cancel("session", "token", "call-1"));
    assert!(cancel.is_cancelled());
    assert!(!calls.cancel("session", "token", "call-1"));
    calls.finish("session", "token", "call-1");
    assert!(calls.begin("session", "token", "call-1").is_err());
}

#[test]
fn abort_session_only_cancels_its_own_calls() {
    let mut calls = Calls::default();
    let first = calls.begin("a", "token", "call").expect("first");
    let second = calls.begin("b", "token", "call").expect("second");
    calls.abort_session("a");
    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
}

#[test]
fn mime_allowlist_matches_exact_and_prefix_entries() {
    assert!(resources::mime_allowed("text/html; charset=utf-8"));
    assert!(resources::mime_allowed("IMAGE/PNG"));
    assert!(resources::mime_allowed("application/octet-stream"));
    assert!(!resources::mime_allowed("application/x-unknown"));
    assert!(!resources::mime_allowed(""));
}

#[test]
fn ui_read_strips_meta_and_rejects_bad_mime_and_missing_blob_mime() {
    let raw = json!({"contents":[{"uri":"ui://s/index.html","mimeType":"text/html","text":"<html/>","_meta":{"x":1}}]});
    let bounded = bound_for_ui("ui://s/index.html", &raw).expect("bounded");
    assert!(bounded["contents"][0].get("_meta").is_none());
    assert_eq!(bounded["contents"][0]["text"], "<html/>");

    let bad =
        json!({"contents":[{"uri":"ui://s/x","mimeType":"application/x-unknown","text":"x"}]});
    assert!(bound_for_ui("ui://s/x", &bad).is_err());

    let blob = json!({"contents":[{"uri":"ui://s/x","blob":"AAAA"}]});
    assert!(bound_for_ui("ui://s/x", &blob).is_err());
}

#[test]
fn ui_read_rejects_oversize_instead_of_truncating() {
    let text = "a".repeat(resources::UI_READ_MAX_BYTES + 1);
    let raw = json!({"contents":[{"uri":"ui://s/big","mimeType":"text/plain","text":text}]});
    assert!(bound_for_ui("ui://s/big", &raw).is_err());
}

#[test]
fn project_resources_keeps_only_standard_descriptors_and_cursor() {
    let raw = json!({
        "resources":[{"uri":"ui://s/a","name":"a","mimeType":"text/html","extra":1,"_meta":{"csp":{}}}],
        "nextCursor":"next"
    });
    let projected = project_resources(&raw);
    assert_eq!(projected["resources"][0]["name"], "a");
    assert!(projected["resources"][0].get("extra").is_none());
    assert_eq!(projected["resources"][0]["_meta"]["csp"], json!({}));
    assert_eq!(projected["nextCursor"], "next");
}

#[test]
fn project_templates_keeps_the_standard_fields() {
    let raw = json!({"resourceTemplates":[{"uriTemplate":"ui://s/{x}","name":"t","description":"d","extra":1}]});
    let projected = project_templates(&raw);
    assert_eq!(
        projected["resourceTemplates"][0]["uriTemplate"],
        "ui://s/{x}"
    );
    assert!(projected["resourceTemplates"][0].get("extra").is_none());
}

#[test]
fn parse_binding_uses_workspace_identity_before_path() {
    let params = json!({
        "workspace":{"workspacePath":"/ignored","workspaceIdentity":"  identity  "},
        "sessionId":"s","pluginId":"p","serverName":"m","scopeId":"scope",
        "resourceUri":"ui://m/x","ownerWebContentsId":3,
        "instance":{"runtimeId":"r","generation":2,"token":"t","appIdentity":"a"}
    });
    let parsed = parse_binding(&params).expect("binding");
    assert_eq!(parsed.workspace, "identity");
    let lease = parse_lease(&params).expect("lease");
    assert_eq!(lease.generation, 2);
    assert_eq!(lease.token, "t");
}

#[test]
fn parse_scope_rejects_missing_server_name() {
    let params = json!({"sessionId":"s","pluginId":"p"});
    assert!(parse_scope(&params).is_err());
}
