//! 远端 scope 装配的单元测试（从 `domains_source_server.rs` 拆出以守住 400 行门禁）。

use super::DEFAULT_REMOTE_SERVER_SCRIPT;

#[test]
fn identity_change_pending_maps_to_structured_rpc_error() {
    let error = anyhow::Error::new(super::IdentityChangePending {
        profile_id: "profile-1".to_owned(),
        pending_source_server_id: "source-2".to_owned(),
    });
    let mapped = super::map_connect_error(error);
    assert_eq!(mapped.name, "Error");
    assert_eq!(
        mapped.extra.get("code"),
        Some(&serde_json::json!("source-server-identity-change-pending"))
    );
    assert_eq!(
        mapped.extra.get("profileId"),
        Some(&serde_json::json!("profile-1"))
    );
    assert_eq!(
        mapped.extra.get("pendingSourceServerId"),
        Some(&serde_json::json!("source-2"))
    );
    assert_eq!(
        mapped.extra.get("retryable"),
        Some(&serde_json::json!(false))
    );
}

#[test]
fn remote_script_uses_the_shared_authority_mode_env_name() {
    // 远端 server 只认 shared 的 ESCODE_SERVICE_AUTHORITY_MODE；写错名字会退回本机权威模式，
    // 导致 provider-provisioning-target 不注册、首连屏障误判 unavailable（S4-g 实测根因）。
    assert!(
        DEFAULT_REMOTE_SERVER_SCRIPT
            .starts_with("ESCODE_SERVICE_AUTHORITY_MODE=\"desktop-attached-remote\"")
    );
}
