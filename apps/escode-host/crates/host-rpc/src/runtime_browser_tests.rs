use super::*;
use crate::runtime_requests::responder;
use escode_host_control::main_broker::MainBroker;
use escode_host_control::{ControlMessage, ControlWriter};
use escode_host_runtime::HostRequest;

#[tokio::test]
async fn browser_execute_forwards_to_main_request() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let broker = MainBroker::new(writer);
    let slot: MainBrokerSlot = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = slot.set(std::sync::Arc::clone(&broker));
    let responder = responder(
        PathBuf::from("/tmp"),
        crate::provider_headers::ProviderHeaders::new(),
        slot,
    );
    let call = tokio::spawn(async move {
        responder(HostRequest {
            identity: "w".to_owned(),
            workspace_path: "/w".to_owned(),
            method: "interaction/browserExecute".to_owned(),
            params: json!({
                "requestId": "r1",
                "sessionId": "s1",
                "command": {"method": "navigate"},
            }),
        })
        .await
    });
    let Some(ControlMessage::MainRequest {
        request_id,
        method,
        params,
    }) = rx.recv().await
    else {
        panic!("expected mainRequest");
    };
    assert_eq!(method, "interaction/browserExecute");
    assert_eq!(params["requestId"], "r1");
    assert!(broker.resolve(&request_id, true, Some(json!({"ok": true})), None));
    assert_eq!(
        call.await.expect("task").expect("result"),
        json!({"ok": true})
    );
}

async fn browser_execute_with_main_response(
    workspace: &Path,
    params: Value,
    response: Value,
) -> Value {
    let (writer, mut rx) = ControlWriter::channel(8);
    let broker = MainBroker::new(writer);
    let slot: MainBrokerSlot = std::sync::Arc::new(std::sync::OnceLock::new());
    let _ = slot.set(std::sync::Arc::clone(&broker));
    let responder = responder(
        PathBuf::from("/tmp"),
        crate::provider_headers::ProviderHeaders::new(),
        slot,
    );
    let workspace_path = workspace.to_string_lossy().into_owned();
    let identity = workspace_path.clone();
    let call = tokio::spawn(async move {
        responder(HostRequest {
            identity,
            workspace_path,
            method: "interaction/browserExecute".to_owned(),
            params,
        })
        .await
    });
    let Some(ControlMessage::MainRequest { request_id, .. }) = rx.recv().await else {
        panic!("expected mainRequest");
    };
    assert!(broker.resolve(&request_id, true, Some(response), None));
    call.await.expect("task").expect("result")
}

fn completed_recording(source: &Path) -> Value {
    json!({
        "ok": true,
        "recording": {
            "id": "rec1",
            "status": "completed",
            "phase": "completed",
            "progress": 1.0,
            "startedAt": 1,
            "updatedAt": 2,
            "artifact": {
                "path": source.to_string_lossy(),
                "mimeType": "video/webm",
                "width": 1280,
                "height": 720,
                "fps": 25.0,
                "durationMs": 1000,
                "frameCount": 25,
            },
        },
        "elapsedMs": 7,
    })
}

#[tokio::test]
async fn browser_recording_materializes_completed_artifact_into_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let temporary = dir.path().join("main-temp.webm");
    std::fs::write(&temporary, b"webm-bytes").unwrap();

    let result = browser_execute_with_main_response(
        &workspace,
        json!({
            "requestId": "r1",
            "sessionId": "s1",
            "workspacePath": workspace.to_string_lossy(),
            "command": {
                "method": "recordingStatus",
                "recordingId": "rec1",
                "outputPath": "recordings/demo.webm",
            },
        }),
        completed_recording(&temporary),
    )
    .await;

    let target = workspace.join("recordings").join("demo.webm");
    assert_eq!(
        result
            .pointer("/recording/artifact/path")
            .and_then(Value::as_str),
        Some(target.to_string_lossy().as_ref())
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"webm-bytes");
    assert!(temporary.exists());
}

#[tokio::test]
async fn browser_recording_without_output_path_strips_temporary_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let temporary = dir.path().join("main-temp.webm");
    std::fs::write(&temporary, b"webm-bytes").unwrap();

    let result = browser_execute_with_main_response(
        &workspace,
        json!({
            "requestId": "r1",
            "sessionId": "s1",
            "workspacePath": workspace.to_string_lossy(),
            "command": {"method": "recordingStatus", "recordingId": "rec1"},
        }),
        completed_recording(&temporary),
    )
    .await;

    assert!(result.pointer("/recording/artifact").is_none());
    assert_eq!(
        result.pointer("/recording/status").and_then(Value::as_str),
        Some("completed")
    );
}

#[tokio::test]
async fn browser_recording_rejects_output_path_escape_and_non_webm() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let temporary = dir.path().join("main-temp.webm");
    std::fs::write(&temporary, b"webm-bytes").unwrap();

    for output_path in [
        "../outside.webm",
        "recordings/../../outside.webm",
        "recordings/demo.mp4",
        "/absolute/demo.webm",
    ] {
        let result = browser_execute_with_main_response(
            &workspace,
            json!({
                "requestId": "r1",
                "sessionId": "s1",
                "workspacePath": workspace.to_string_lossy(),
                "command": {
                    "method": "recordingStatus",
                    "recordingId": "rec1",
                    "outputPath": output_path,
                },
            }),
            completed_recording(&temporary),
        )
        .await;

        assert_eq!(result["ok"], json!(false), "{output_path}");
        assert_eq!(
            result.pointer("/error/code").and_then(Value::as_str),
            Some("execution_error"),
            "{output_path}"
        );
        assert_eq!(
            result.pointer("/error/sideEffect").and_then(Value::as_str),
            Some("none"),
            "{output_path}"
        );
    }
    assert!(!dir.path().join("outside.webm").exists());
    assert!(!workspace.join("recordings").exists());
}

#[tokio::test]
async fn browser_recording_rejects_remote_session_materialization() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let temporary = dir.path().join("main-temp.webm");
    std::fs::write(&temporary, b"webm-bytes").unwrap();

    let result = browser_execute_with_main_response(
        &workspace,
        json!({
            "requestId": "r1",
            "sessionId": "s1",
            "workspacePath": workspace.to_string_lossy(),
            "remoteSessionId": "remote-1",
            "command": {
                "method": "recordingStatus",
                "recordingId": "rec1",
                "outputPath": "recordings/demo.webm",
            },
        }),
        completed_recording(&temporary),
    )
    .await;

    assert_eq!(result["ok"], json!(false));
    assert_eq!(
        result.pointer("/error/message").and_then(Value::as_str),
        Some("remote Browser recording materialization is unavailable for this session")
    );
    assert!(!workspace.join("recordings").exists());
}
