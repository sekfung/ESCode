//! Rust Host 的浏览器命令转发与录制产物物化。
//!
//! Main 只持有 WebView 与短期 WebM；workspace 写入权归 Host。`recordingStatus` 带 `outputPath`
//! 时必须在回包前落盘并改写 path，避免 Agent 拿到 main 临时路径
//! （docs/browser-use/2026-08-12-browser-webview-video-recording-spec.md §8）。

use std::path::{Path, PathBuf};
use std::time::Instant;

use escode_host_runtime::HostError;
use serde_json::{Value, json};

use crate::runtime_requests::MainBrokerSlot;

pub(crate) async fn browser_execute(
    main_broker: &MainBrokerSlot,
    fallback_workspace_path: &str,
    params: &Value,
) -> Result<Value, HostError> {
    if params.get("requestId").and_then(Value::as_str).is_none()
        || params.get("sessionId").and_then(Value::as_str).is_none()
        || params.get("command").is_none()
    {
        return Err(HostError {
            code: -32602,
            message: "Invalid interaction/browserExecute params".to_owned(),
        });
    }
    let Some(broker) = main_broker.get() else {
        return Ok(browser_unavailable("browser control not available"));
    };
    let started_at = Instant::now();
    let result = match broker
        .request("interaction/browserExecute", params.clone())
        .await
    {
        Ok(result) => result,
        Err(error) => return Ok(browser_unavailable(&error)),
    };
    Ok(finish_browser_result(result, params, fallback_workspace_path, started_at).await)
}

/// Main 只持有短期录制产物；Host 是 workspace 的唯一写者。`recordingStatus` 带 `outputPath` 时
/// 必须在回包前落盘并改写 path，否则 Agent 会拿到 main 的临时路径（BVR06）。
async fn finish_browser_result(
    mut result: Value,
    params: &Value,
    fallback_workspace_path: &str,
    started_at: Instant,
) -> Value {
    let completed = result.get("ok").and_then(Value::as_bool) == Some(true)
        && result.pointer("/recording/status").and_then(Value::as_str) == Some("completed");
    let Some(artifact_path) = result
        .pointer("/recording/artifact/path")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return result;
    };
    if !completed {
        return result;
    }
    let output_path = params
        .pointer("/command/outputPath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(output_path) = output_path else {
        // 与 Node Host 一致：未请求 outputPath 时不把 main 临时 artifact 暴露给 Agent。
        if let Some(recording) = result.get_mut("recording").and_then(Value::as_object_mut) {
            recording.remove("artifact");
        }
        return result;
    };
    if params
        .get("remoteSessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return browser_execution_error(
            "remote Browser recording materialization is unavailable for this session",
            started_at,
        );
    }
    let workspace_path = params
        .get("workspacePath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_workspace_path);
    match materialize_recording_artifact(workspace_path, output_path, &artifact_path).await {
        Ok(path) => {
            if let Some(artifact) = result
                .get_mut("recording")
                .and_then(Value::as_object_mut)
                .and_then(|recording| recording.get_mut("artifact"))
                .and_then(Value::as_object_mut)
            {
                artifact.insert("path".to_owned(), Value::String(path));
            }
            result
        }
        Err(message) => browser_execution_error(&message, started_at),
    }
}

/// 与 shared `browserRecordingOutputPathSchema` 同一规则：Host 不能只信任 runtime 已校验。
fn recording_output_segments(output_path: &str) -> Result<Vec<&str>, String> {
    let value = output_path.trim();
    let bytes = value.as_bytes();
    if value.is_empty()
        || value.len() > 2_000
        || matches!(bytes.first().copied(), Some(b'/') | Some(b'\\'))
        || (bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':'))
    {
        return Err("recording outputPath must stay inside the workspace".to_owned());
    }
    let segments: Vec<&str> = value.split(['/', '\\']).collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        return Err("recording outputPath must stay inside the workspace".to_owned());
    }
    if !value.to_ascii_lowercase().ends_with(".webm") {
        return Err("recording outputPath must end with .webm".to_owned());
    }
    Ok(segments)
}

async fn materialize_recording_artifact(
    workspace_path: &str,
    output_path: &str,
    artifact_path: &str,
) -> Result<String, String> {
    let segments = recording_output_segments(output_path)?;
    if workspace_path.trim().is_empty() {
        return Err("browser recording workspacePath is missing".to_owned());
    }
    if artifact_path.trim().is_empty() {
        return Err("browser recording artifact is missing its temporary path".to_owned());
    }
    let source = Path::new(artifact_path);
    let mut target = PathBuf::from(workspace_path.trim());
    for segment in segments {
        target.push(segment);
    }
    let parent = target
        .parent()
        .ok_or_else(|| "recording outputPath must stay inside the workspace".to_owned())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| format!("browser recording artifact materialization failed: {error}"))?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("recording.webm");
    let staging = parent.join(format!(
        "{file_name}.escode-recording-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let committed = async {
        tokio::fs::copy(source, &staging).await?;
        // Windows 不能用 rename 原子覆盖已有文件；先移除明确的目标 WebM，再提交 staging。
        match tokio::fs::remove_file(&target).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        tokio::fs::rename(&staging, &target).await
    }
    .await;
    let _ = tokio::fs::remove_file(&staging).await;
    committed
        .map_err(|error| format!("browser recording artifact materialization failed: {error}"))?;
    Ok(target.to_string_lossy().into_owned())
}

fn browser_execution_error(message: &str, started_at: Instant) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": "execution_error",
            "message": message,
            "sideEffect": "none",
        },
        "elapsedMs": started_at.elapsed().as_millis() as u64,
    })
}

fn browser_unavailable(message: &str) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": "backend_unavailable",
            "message": message,
            "sideEffect": "uncertain",
        },
        "elapsedMs": 0,
    })
}

#[cfg(test)]
#[path = "runtime_browser_tests.rs"]
mod tests;
