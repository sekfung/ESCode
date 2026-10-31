use serde_json::{Value, json};

use super::*;
use crate::source_server_release::InstallPreview;

fn empty_snapshot() -> Value {
    json!({
        "generation": 1,
        "profiles": [],
        "servers": [],
        "workspaceHistory": [],
        "states": [],
        "failedPathCount": 0,
        "abandonedPathCount": 0,
        "migrationCounts": [],
    })
}

#[test]
fn empty_settings_snapshot_is_the_contract_shape() {
    let registry = SourceServerRegistry::from_settings(&json!({})).expect("registry");
    assert_eq!(registry.snapshot().expect("snapshot"), empty_snapshot());
}

#[test]
fn null_settings_values_are_treated_as_empty() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "sourceServers": null,
        "connectionProfiles": null,
        "sourceServerWorkspaceHistory": null,
    }))
    .expect("registry");
    assert_eq!(registry.snapshot().expect("snapshot"), empty_snapshot());
}

#[test]
fn records_are_projected_without_unknown_keys() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "sourceServers": [{
            "sourceServerId": "11111111-1111-4111-8111-111111111111",
            "displayName": "build-box",
            "declaredWorkspaces": [],
            "unknownFutureField": true,
        }],
        "connectionProfiles": [{
            "profileId": "22222222-2222-4222-8222-222222222222",
            "sourceServerId": "11111111-1111-4111-8111-111111111111",
            "transport": "ssh",
            "target": {"kind": "ssh", "host": "build.example", "username": "dev"},
            "createdAt": 1_700_000_000_000_u64,
            "lastConnectedAt": 1_700_000_100_000_u64,
            "unknownFutureField": 1,
        }],
        "sourceServerWorkspaceHistory": [{
            "workspacePath": "/srv/app",
            "profileId": "22222222-2222-4222-8222-222222222222",
            "lastOpenedAt": 1_700_000_200_000_u64,
        }],
    }))
    .expect("registry");
    let snapshot = registry.snapshot().expect("snapshot");

    assert_eq!(snapshot["generation"], json!(1));
    assert_eq!(snapshot["servers"][0]["displayName"], json!("build-box"));
    assert_eq!(snapshot["servers"][0]["installState"], json!("installed"));
    assert!(snapshot["servers"][0].get("unknownFutureField").is_none());
    assert!(snapshot["profiles"][0].get("unknownFutureField").is_none());
    assert_eq!(snapshot["profiles"][0]["lifecycleState"], json!("active"));
    assert_eq!(
        snapshot["workspaceHistory"][0]["workspacePath"],
        json!("/srv/app")
    );
    assert_eq!(
        snapshot["states"],
        json!([{
            "profileId": "22222222-2222-4222-8222-222222222222",
            "sourceServerId": "11111111-1111-4111-8111-111111111111",
            "state": "absent",
            "updatedAt": 1_700_000_100_000_u64,
        }])
    );
}

#[test]
fn last_connect_error_projects_failed_state() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "connectionProfiles": [{
            "profileId": "22222222-2222-4222-8222-222222222222",
            "transport": "ssh",
            "target": {"kind": "ssh", "host": "build.example", "username": "dev"},
            "createdAt": 1_u64,
            "lastConnectError": "handshake failed",
        }],
    }))
    .expect("registry");
    assert_eq!(
        registry.snapshot().expect("snapshot")["states"],
        json!([{
            "profileId": "22222222-2222-4222-8222-222222222222",
            "state": "failed",
            "error": "handshake failed",
            "updatedAt": 0,
        }])
    );
}

#[test]
fn recent_operation_logs_unknown_profile_is_empty() {
    let registry = SourceServerRegistry::empty();
    assert_eq!(
        registry.recent_operation_logs("missing"),
        Vec::<Value>::new()
    );
}

#[test]
fn load_without_settings_file_is_empty() {
    let dir = tempfile::tempdir().expect("dir");
    let registry = SourceServerRegistry::load(dir.path());
    assert_eq!(registry.snapshot().expect("snapshot"), empty_snapshot());
}

#[test]
fn load_with_corrupt_settings_reports_explicit_error() {
    let dir = tempfile::tempdir().expect("dir");
    let path = settings::settings_path(dir.path());
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, "{not json").expect("write");

    let registry = SourceServerRegistry::load(dir.path());
    let error = registry.snapshot().expect_err("load error must surface");
    assert!(error.contains("parse"), "unexpected error: {error}");
}

#[test]
fn non_array_entries_are_a_parse_error() {
    let result = SourceServerRegistry::from_settings(&json!({"connectionProfiles": 7}));
    let Err(error) = result else {
        panic!("expected a parse error");
    };
    assert!(format!("{error:#}").contains("connectionProfiles"));
}

#[test]
fn compatibility_fields_are_serialized_without_nulls() {
    let profile_id = "22222222-2222-4222-8222-222222222222";
    let registry = SourceServerRegistry::from_settings(&json!({
        "connectionProfiles": [{
            "profileId": profile_id,
            "transport": "ssh",
            "target": {"kind": "ssh", "host": "build.example", "username": "dev"},
            "createdAt": 1_u64,
        }],
    }))
    .expect("registry");

    let mut state = ConnectionStateSnapshot::new(profile_id, None, "online", 42);
    state.current_version = Some("3.15.2".to_owned());
    state.update_availability = Some("available");
    state.available_version = Some("3.16.0".to_owned());
    state.minimum_version = Some("3.15.0".to_owned());
    state.upgrade_target = Some("server");
    registry.set_connection_state(state);

    let snapshot = registry.snapshot().expect("snapshot");
    let state = &snapshot["states"][0];
    assert_eq!(state["updateAvailability"], json!("available"));
    assert_eq!(state["availableVersion"], json!("3.16.0"));
    assert_eq!(state["minimumVersion"], json!("3.15.0"));
    assert_eq!(state["upgradeTarget"], json!("server"));

    registry.set_connection_state(ConnectionStateSnapshot::new(profile_id, None, "online", 43));
    let snapshot = registry.snapshot().expect("snapshot");
    for key in [
        "updateAvailability",
        "availableVersion",
        "minimumVersion",
        "upgradeTarget",
    ] {
        assert!(
            snapshot["states"][0].get(key).is_none(),
            "unknown compatibility field {key} must be omitted, not null"
        );
    }
}

#[test]
fn install_preview_is_serialized_without_optional_nulls() {
    let profile_id = "22222222-2222-4222-8222-222222222222";
    let registry = SourceServerRegistry::from_settings(&json!({
        "connectionProfiles": [{
            "profileId": profile_id,
            "transport": "ssh",
            "target": {"kind": "ssh", "host": "build.example", "username": "dev"},
            "createdAt": 1_u64,
        }],
    }))
    .expect("registry");

    let mut state = ConnectionStateSnapshot::new(profile_id, None, "needs-install", 42);
    state.install_preview = Some(InstallPreview {
        version: "3.15.2".to_owned(),
        install_path: None,
        size_bytes: 42,
    });
    registry.set_connection_state(state);

    let snapshot = registry.snapshot().expect("snapshot");
    assert_eq!(
        snapshot["states"][0]["installPreview"],
        json!({"version": "3.15.2", "sizeBytes": 42})
    );
}

#[test]
fn operation_logs_are_retained_and_broadcast() {
    let profile_id = "22222222-2222-4222-8222-222222222222";
    let registry = SourceServerRegistry::from_settings(&json!({})).expect("registry");
    let mut events = registry.subscribe();

    registry.append_operation_log(ConnectionOperationLog::new(
        profile_id,
        "connect",
        "connecting",
        "info",
        "connecting",
        1,
    ));
    registry.append_operation_log(ConnectionOperationLog::new(
        profile_id, "connect", "online", "info", "online", 2,
    ));

    let logs = registry.recent_operation_logs(profile_id);
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1]["phase"], json!("online"));
    assert_eq!(logs[1]["operation"], json!("connect"));
    assert!(logs[1].get("progress").is_none());
    assert!(registry.recent_operation_logs("unknown").is_empty());

    match events.try_recv().expect("event") {
        SourceServerEvent::OperationLog(value) => {
            assert_eq!(value["profileId"], json!(profile_id));
            assert_eq!(value["message"], json!("connecting"));
        }
        other => panic!("expected operation log, got {other:?}"),
    }
}

#[test]
fn startup_candidates_select_recent_online_active_preferred_profiles() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "sourceServers": [
            {
                "sourceServerId": "s1",
                "displayName": "preferred",
                "lastOnlineAt": 1_000_000_u64,
                "preferredProfileId": "p1",
            },
            {
                "sourceServerId": "s2",
                "displayName": "stale",
                "lastOnlineAt": 1_000_u64,
                "preferredProfileId": "p3",
            },
            {
                "sourceServerId": "s3",
                "displayName": "no-profile",
                "lastOnlineAt": 900_000_u64,
            },
            {
                "sourceServerId": "s4",
                "displayName": "latest",
                "lastOnlineAt": 2_000_000_u64,
                "preferredProfileId": "p4",
            },
            {
                "sourceServerId": "s5",
                "displayName": "retired",
                "lastOnlineAt": 1_500_000_u64,
                "preferredProfileId": "p5",
            }
        ],
        "connectionProfiles": [
            {"profileId": "p1", "sourceServerId": "s1", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64, "lastConnectedAt": 10_u64},
            {"profileId": "p2", "sourceServerId": "s1", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64, "lastConnectedAt": 20_u64},
            {"profileId": "p3", "sourceServerId": "s2", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64},
            {"profileId": "p4", "sourceServerId": "s4", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64},
            {"profileId": "p5", "sourceServerId": "s5", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64, "lifecycleState": "retired"}
        ],
    }))
    .expect("registry");

    let candidates = registry.startup_candidates(2_100_000, 1_200_000);
    assert_eq!(
        candidates,
        vec![
            StartupReconnectCandidate {
                profile_id: "p4".to_owned(),
                source_server_id: "s4".to_owned(),
                last_online_at: 2_000_000,
            },
            StartupReconnectCandidate {
                profile_id: "p1".to_owned(),
                source_server_id: "s1".to_owned(),
                last_online_at: 1_000_000,
            },
        ],
        "过期/无活动 Profile 的 Source 被排除，preferred Profile 优先，按最近在线倒序"
    );
}

#[test]
fn startup_candidates_fall_back_to_most_recently_connected_profile() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "sourceServers": [{
            "sourceServerId": "s1",
            "displayName": "fallback",
            "lastOnlineAt": 500_u64,
        }],
        "connectionProfiles": [
            {"profileId": "old", "sourceServerId": "s1", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64, "lastConnectedAt": 10_u64},
            {"profileId": "new", "sourceServerId": "s1", "transport": "ssh",
             "target": {"kind": "ssh", "host": "h", "username": "u"},
             "createdAt": 1_u64, "lastConnectedAt": 20_u64}
        ],
    }))
    .expect("registry");

    let candidates = registry.startup_candidates(500, 1_000);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].profile_id, "new");
}

#[test]
fn last_online_advances_on_online_entries_and_exits_only() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "sourceServers": [{
            "sourceServerId": "s1",
            "displayName": "clock",
            "lastOnlineAt": 50_u64,
        }],
        "connectionProfiles": [{
            "profileId": "p1",
            "sourceServerId": "s1",
            "transport": "ssh",
            "target": {"kind": "ssh", "host": "h", "username": "u"},
            "createdAt": 1_u64,
        }],
    }))
    .expect("registry");

    let online = ConnectionStateSnapshot::new("p1", Some("s1"), "online", 100);
    registry.set_connection_state(online.clone());
    assert_eq!(
        registry.source_server("s1").unwrap().last_online_at,
        Some(100)
    );

    let mut retrying = online.clone();
    retrying.state = "retrying";
    retrying.updated_at = 200;
    registry.set_connection_state(retrying);
    assert_eq!(
        registry.source_server("s1").unwrap().last_online_at,
        Some(200),
        "离开 online 时以 Host 时钟推进"
    );

    let mut failed = online;
    failed.state = "failed";
    failed.updated_at = 300;
    registry.set_connection_state(failed);
    assert_eq!(
        registry.source_server("s1").unwrap().last_online_at,
        Some(200),
        "离线重试/失败不推进 lastOnlineAt"
    );
}

#[test]
fn last_online_writes_are_monotonic_and_never_recreate_a_deleted_source() {
    let registry = SourceServerRegistry::from_settings(&json!({
        "sourceServers": [{
            "sourceServerId": "s1",
            "displayName": "clock",
            "lastOnlineAt": 50_u64,
        }],
    }))
    .expect("registry");

    registry.record_last_online_now("s1", 40);
    assert_eq!(
        registry.source_server("s1").unwrap().last_online_at,
        Some(50)
    );
    registry.record_last_online_now("s1", 60);
    assert_eq!(
        registry.source_server("s1").unwrap().last_online_at,
        Some(60)
    );
    registry.record_last_online_now("deleted", 70);
    assert!(registry.source_server("deleted").is_none());
    assert_eq!(
        registry.snapshot().unwrap()["servers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn recovery_fields_serialize_in_camel_case() {
    let mut state = ConnectionStateSnapshot::new("p1", Some("s1"), "offline", 10);
    state.recovery_attempt = Some(3);
    state.recovery_limit = Some(3);
    state.recovery_budget_exhausted = Some(true);
    let value = serde_json::to_value(&state).expect("serialize");
    assert_eq!(value["recoveryAttempt"], json!(3));
    assert_eq!(value["recoveryLimit"], json!(3));
    assert_eq!(value["recoveryBudgetExhausted"], json!(true));
}
