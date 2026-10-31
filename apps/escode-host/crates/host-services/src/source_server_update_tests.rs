use serde_json::{Value, json};

use super::*;
use crate::source_server::ConnectionStateSnapshot;

fn profile(transport: &str, target: Value) -> ConnectionProfile {
    ConnectionProfile {
        profile_id: "44444444-4444-4444-8444-444444444444".to_owned(),
        transport: transport.to_owned(),
        target,
        source_server_id: None,
        display_name: None,
        last_connected_at: None,
        last_connect_error: None,
        created_at: 1,
        lifecycle_state: "active".to_owned(),
        retired_reason: None,
        retired_at: None,
    }
}

#[test]
fn expected_target_matches_normalizes_ssh_host_and_default_port() {
    let profile = profile(
        "ssh",
        json!({"kind": "ssh", "host": "Build.Example", "username": "dev"}),
    );
    let expected = json!({
        "kind": "ssh",
        "host": "build.example",
        "port": 22,
        "username": "dev",
    });
    assert!(expected_target_matches(&profile, &expected).expect("match"));
}

#[test]
fn expected_target_matches_rejects_changed_identity() {
    let profile = profile(
        "ssh",
        json!({"kind": "ssh", "host": "build.example", "port": 2222, "username": "dev"}),
    );
    for expected in [
        json!({"kind": "ssh", "host": "other.example", "port": 2222, "username": "dev"}),
        json!({"kind": "ssh", "host": "build.example", "port": 22, "username": "dev"}),
        json!({"kind": "ssh", "host": "build.example", "port": 2222, "username": "root"}),
        json!({"kind": "docker", "container": "same-name"}),
    ] {
        assert!(
            !expected_target_matches(&profile, &expected).expect("match"),
            "expected {expected} to mismatch"
        );
    }
}

#[test]
fn expected_target_matches_requires_wsl_identity() {
    let profile = profile(
        "wsl",
        json!({"kind": "wsl", "distro": "Ubuntu", "user": "dev"}),
    );
    assert!(
        expected_target_matches(
            &profile,
            &json!({"kind": "wsl", "distro": "Ubuntu", "user": "dev"})
        )
        .expect("match")
    );
    assert!(
        !expected_target_matches(&profile, &json!({"kind": "wsl"})).expect("match"),
        "missing WSL identity must not be treated as the default user"
    );
}

#[test]
fn update_request_is_strict() {
    let request = UpdateRequest::from_json(&json!({
        "profileId": "44444444-4444-4444-8444-444444444444",
        "force": true,
        "expectedTarget": {"kind": "docker", "container": "c"},
    }))
    .expect("request");
    assert!(request.force);
    assert!(request.expected_target.is_some());
    assert!(UpdateRequest::from_json(&json!({"profileId": "", "force": false})).is_err());
    assert!(
        UpdateRequest::from_json(&json!({
            "profileId": "p",
            "force": false,
            "unknown": true,
        }))
        .is_err()
    );
    assert!(UpdateRequest::from_json(&json!({"profileId": "p", "force": "yes"})).is_err());
}

#[test]
fn parse_running_tasks_keeps_only_active_rows() {
    let value = json!({
        "items": [
            {"taskId": "a", "workspacePath": "/w", "status": "running"},
            {"taskId": "b", "workspacePath": "/w", "status": "completed"},
            {"taskId": "c", "workspacePath": "/w", "workspaceIdentity": "id", "status": "running"},
        ],
        "total": 3,
        "hasMore": false,
    });
    let tasks = parse_running_tasks(&value).expect("tasks");
    assert_eq!(
        tasks,
        vec![
            RunningTask {
                session_id: "a".to_owned(),
                workspace_path: "/w".to_owned(),
                workspace_identity: None,
            },
            RunningTask {
                session_id: "c".to_owned(),
                workspace_path: "/w".to_owned(),
                workspace_identity: Some("id".to_owned()),
            },
        ]
    );
}

#[test]
fn parse_running_tasks_rejects_incomplete_running_row() {
    let value = json!({
        "items": [{"taskId": "a", "status": "running"}],
    });
    assert!(parse_running_tasks(&value).is_err());
    assert!(parse_running_tasks(&json!({"total": 0})).is_err());
}

#[test]
fn task_list_request_scopes_declared_workspaces() {
    let request = task_list_request(&[DeclaredWorkspace {
        workspace_path: "/remote/workspace".to_owned(),
        workspace_identity: "remote:/remote/workspace".to_owned(),
        label: None,
    }]);
    assert_eq!(request["kind"], json!("active"));
    assert_eq!(
        request["workspaceScopes"][0]["workspacePath"],
        json!("/remote/workspace")
    );
    assert_eq!(
        request["workspaceScopes"][0]["workspaceIdentity"],
        json!("remote:/remote/workspace")
    );
    assert!(request["limit"].is_null());
}

#[test]
fn update_failure_state_preserves_online_and_attaches_tasks() {
    let previous = ConnectionStateSnapshot::new("p", Some("s"), "online", 10);
    let state = update_failure_state(
        &previous,
        "busy".to_owned(),
        Some("source-server-update-running-tasks"),
        Some(false),
        Some(vec![RunningTask {
            session_id: "a".to_owned(),
            workspace_path: "/w".to_owned(),
            workspace_identity: None,
        }]),
        20,
    );
    assert_eq!(state.state, "online");
    assert_eq!(state.source_server_id.as_deref(), Some("s"));
    assert_eq!(state.error.as_deref(), Some("busy"));
    assert_eq!(
        state.error_code.as_deref(),
        Some("source-server-update-running-tasks")
    );
    assert_eq!(state.retryable, Some(false));
    assert_eq!(state.running_tasks.expect("tasks").len(), 1);
    assert_eq!(state.updated_at, 20);
}

#[test]
fn updating_state_keeps_current_version_and_preview() {
    let mut previous = ConnectionStateSnapshot::new("p", Some("s"), "online", 10);
    previous.current_version = Some("3.14.0".to_owned());
    let profile = profile("docker", json!({"kind": "docker", "container": "c"}));
    let preview = InstallPreview {
        version: "3.15.2".to_owned(),
        install_path: Some("/home/dev/.escode/server".to_owned()),
        size_bytes: 42,
    };
    let state = updating_state(&profile, &previous, &Some(preview.clone()), None, 30);
    assert_eq!(state.state, "updating");
    assert_eq!(state.current_version.as_deref(), Some("3.14.0"));
    assert_eq!(state.install_preview, Some(preview));
    assert_eq!(state.updated_at, 30);
}
