use serde_json::json;

use super::*;

fn body_of(service: &str, params: serde_json::Value) -> serde_json::Value {
    let spec = SPECS.iter().find(|s| s.service == service).expect("spec");
    spec.body(
        &params,
        &json!({"workspacePath": "/w", "workspaceKey": "/w"}),
    )
}

#[test]
fn every_spec_has_a_unique_service_and_runtime_method() {
    let mut services = std::collections::BTreeSet::new();
    let mut runtimes = std::collections::BTreeSet::new();
    for spec in SPECS {
        assert!(services.insert(spec.service), "{}", spec.service);
        assert!(runtimes.insert(spec.runtime), "{}", spec.runtime);
        assert!(spec.runtime.starts_with("plugins/"));
    }
    assert_eq!(SPECS.len(), 16);
}

#[test]
fn list_includes_config_scope_only_when_truthy() {
    let w = json!({"workspacePath": "/w", "workspaceKey": "/w"});
    assert_eq!(
        body_of("listPlugins", json!({"workspacePath": "/w"})),
        json!({"workspace": w})
    );
    assert_eq!(
        body_of("listPlugins", json!({"configScope": "user"})),
        json!({"workspace": w, "configScope": "user"})
    );
    assert_eq!(
        body_of("listPlugins", json!({"configScope": ""})),
        json!({"workspace": w})
    );
    assert_eq!(
        body_of("listPlugins", json!({"configScope": null})),
        json!({"workspace": w})
    );
}

#[test]
fn install_keeps_dry_run_false_but_drops_empty_operation_id() {
    let body = body_of(
        "installPlugin",
        json!({"pluginName": "p", "marketplace": "m", "dryRun": false, "operationId": "", "scope": ""}),
    );
    assert_eq!(body["dryRun"], false, "dryRun 只要不是 undefined 就带上");
    assert!(body.get("operationId").is_none());
    assert!(body.get("scope").is_none());
    assert_eq!(body["pluginName"], "p");
}

#[test]
fn configure_drops_an_empty_clear_list_but_keeps_options() {
    let body = body_of(
        "configurePlugin",
        json!({"pluginId": "p@m", "options": {}, "clearOptionKeys": [], "dryRun": null}),
    );
    assert!(
        body.get("clearOptionKeys").is_none(),
        "?.length 为 0 → 省略"
    );
    assert_eq!(body["options"], json!({}));
    assert_eq!(body["dryRun"], json!(null), "null !== undefined → 保留");
    let body = body_of(
        "configurePlugin",
        json!({"pluginId": "p", "options": {}, "clearOptionKeys": ["a"]}),
    );
    assert_eq!(body["clearOptionKeys"], json!(["a"]));
}

#[test]
fn cancel_has_no_workspace_and_a_short_timeout() {
    let spec = SPECS
        .iter()
        .find(|s| s.service == "cancelPluginOperation")
        .expect("spec");
    assert!(!spec.with_workspace);
    assert_eq!(spec.timeout, Duration::from_secs(5));
    assert_eq!(
        body_of("cancelPluginOperation", json!({"operationId": "op"})),
        json!({"operationId": "op"})
    );
}

#[test]
fn uninstall_and_set_enabled_follow_the_ts_conditions() {
    let body = body_of(
        "uninstallPlugin",
        json!({"pluginId": "", "pluginName": "n", "removeCache": false}),
    );
    assert!(body.get("pluginId").is_none());
    assert_eq!(body["pluginName"], "n");
    assert_eq!(body["removeCache"], false);
    let body = body_of(
        "setPluginEnabled",
        json!({"pluginId": "p", "enabled": false, "operationId": "o"}),
    );
    assert_eq!(body["enabled"], false, "enabled 是必带键");
    assert_eq!(body["operationId"], "o");
}

#[test]
fn catalog_body_includes_session_id_only_when_present() {
    let w = json!({"workspacePath": "/w"});
    assert_eq!(catalog_body(&json!({}), &w), json!({"workspace": w}));
    assert_eq!(
        catalog_body(&json!({"sessionId": "s"}), &w),
        json!({"workspace": w, "sessionId": "s"})
    );
    assert_eq!(
        catalog_body(&json!({"sessionId": ""}), &w),
        json!({"workspace": w})
    );
}
