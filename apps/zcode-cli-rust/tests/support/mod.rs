//! 进程内 Engine 测试的共享 Host 桩。
use serde_json::{Value, json};
use tokio::sync::mpsc;
use zcode_cli_rust::contract::Input;

/// Host 对 runtime 偏好反向请求的最小应答（Memory 关闭、shell 自动探测），其余输出按批转发。
pub fn answer_runtime_preferences(
    mut raw: mpsc::Receiver<Vec<Value>>,
    tx: mpsc::Sender<Input>,
) -> mpsc::Receiver<Vec<Value>> {
    let (forward, output) = mpsc::channel(20);
    tokio::spawn(async move {
        while let Some(batch) = raw.recv().await {
            let mut rest = Vec::with_capacity(batch.len());
            for message in batch {
                if message["method"] == "session/requestRuntimePreferences" {
                    let _ = tx
                        .send(Input::Response {
                            id: message["id"].as_str().unwrap_or_default().into(),
                            result: json!({}),
                            error: None,
                            raw_result: None,
                        })
                        .await;
                } else {
                    rest.push(message);
                }
            }
            // 已应答的偏好请求不转发，测试看到的输出批次与未引入该请求前一致。
            if !rest.is_empty() && forward.send(rest).await.is_err() {
                break;
            }
        }
    });
    output
}
