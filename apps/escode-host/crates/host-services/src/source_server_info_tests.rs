use serde_json::json;

use super::{
    SERVER_REMOTE_PROTOCOL_VERSION, ServerRemoteWorkspace, parse_host_capability, parse_server_info,
};

fn valid_info() -> serde_json::Value {
    json!({
        "serverId": "srv-1",
        "name": " Studio ",
        "version": "3.16.2",
        "protocolVersion": 1,
        "authRequired": true,
        "workspaces": [
            {"path": "/home/dev/project", "label": "project", "workspaceIdentity": "remote:dev"}
        ],
        "capabilities": {"desktopContinuous": true, "websocketRpc": true},
    })
}

#[test]
fn parses_full_server_info() {
    let info = parse_server_info(&valid_info()).expect("server info");
    assert_eq!(info.server_id, "srv-1");
    assert_eq!(info.name.as_deref(), Some("Studio"));
    assert_eq!(info.version, "3.16.2");
    assert_eq!(info.protocol_version, SERVER_REMOTE_PROTOCOL_VERSION);
    assert!(info.auth_required);
    assert_eq!(
        info.workspaces,
        vec![ServerRemoteWorkspace {
            path: "/home/dev/project".to_owned(),
            label: Some("project".to_owned()),
            workspace_identity: Some("remote:dev".to_owned()),
        }]
    );
}

#[test]
fn name_and_workspace_metadata_are_optional() {
    let mut value = valid_info();
    value.as_object_mut().expect("object").remove("name");
    value["workspaces"] = json!([{"path": "/w"}]);
    let info = parse_server_info(&value).expect("server info");
    assert_eq!(info.name, None);
    assert_eq!(
        info.workspaces,
        vec![ServerRemoteWorkspace {
            path: "/w".to_owned(),
            label: None,
            workspace_identity: None,
        }]
    );
}

#[test]
fn blank_optional_text_is_treated_as_absent() {
    let mut value = valid_info();
    value["name"] = json!("   ");
    value["workspaces"] = json!([{"path": "/w", "label": " ", "workspaceIdentity": ""}]);
    let info = parse_server_info(&value).expect("server info");
    assert_eq!(info.name, None);
    assert_eq!(info.workspaces[0].label, None);
    assert_eq!(info.workspaces[0].workspace_identity, None);
}

#[test]
fn rejects_unknown_protocol_version() {
    let mut value = valid_info();
    value["protocolVersion"] = json!(2);
    let error = parse_server_info(&value).expect_err("protocol");
    assert!(
        error.to_string().contains("source-server-incompatible"),
        "{error}"
    );
}

#[test]
fn rejects_missing_or_disabled_capabilities() {
    let mut missing = valid_info();
    missing
        .as_object_mut()
        .expect("object")
        .remove("capabilities");
    let error = parse_server_info(&missing).expect_err("missing capabilities");
    assert!(
        error.to_string().contains("missing capabilities"),
        "{error}"
    );

    for key in ["desktopContinuous", "websocketRpc"] {
        let mut value = valid_info();
        value["capabilities"][key] = json!(false);
        let error = parse_server_info(&value).expect_err("disabled capability");
        assert!(
            error.to_string().contains("source-server-incompatible"),
            "{key}: {error}"
        );
    }
}

#[test]
fn rejects_missing_required_fields() {
    for key in [
        "serverId",
        "version",
        "protocolVersion",
        "authRequired",
        "workspaces",
    ] {
        let mut value = valid_info();
        value.as_object_mut().expect("object").remove(key);
        assert!(parse_server_info(&value).is_err(), "{key} 缺失必须失败");
    }
    let mut value = valid_info();
    value["workspaces"] = json!([{}]);
    assert!(
        parse_server_info(&value).is_err(),
        "workspace 缺 path 必须失败"
    );
}

#[test]
fn parses_host_capability() {
    assert_eq!(
        parse_host_capability(&json!({"capability": "ticket", "expiresAt": 1700000000000i64}))
            .expect("capability"),
        "ticket"
    );
    assert!(
        parse_host_capability(&json!({"capability": "ticket", "expiresAt": 0})).is_err(),
        "expiresAt 必须为正"
    );
    assert!(
        parse_host_capability(&json!({"expiresAt": 1})).is_err(),
        "缺 capability 必须失败"
    );
    assert!(
        parse_host_capability(&json!({"capability": "  ", "expiresAt": 1})).is_err(),
        "空白 capability 必须失败"
    );
}
