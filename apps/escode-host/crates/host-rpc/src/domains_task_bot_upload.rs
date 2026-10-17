//! Bot group 附件上传（TS `groupAttachmentUpload.ts`）：内容寄存在目标 CLI，
//! 群队列只保存 attachmentRef。

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::agent_v4::base::Base;
use crate::forward::RuntimePolicy;
use crate::session::RpcError;

const CHUNK_BYTES: usize = 384 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

pub(super) async fn upload_attachments(
    base: &Arc<Base>,
    target: &Value,
    connection_id: &str,
    attachments: &[Value],
) -> Result<Vec<Value>, RpcError> {
    let mut refs = Vec::new();
    for attachment in attachments {
        let data = match attachment["dataBase64"].as_str() {
            Some(data) => base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|error| {
                    RpcError::new("Error", format!("Invalid attachment data: {error}"))
                })?,
            None => match attachment["localPath"].as_str() {
                Some(path) => std::fs::read(path).map_err(|error| {
                    RpcError::new(
                        "Error",
                        format!("Attachment content is unavailable: {error}"),
                    )
                })?,
                None => {
                    if attachment["kind"] == "file" {
                        attachment["textContent"]
                            .as_str()
                            .map(|text| text.as_bytes().to_vec())
                            .ok_or_else(|| {
                                RpcError::new("Error", "Attachment content is unavailable")
                            })?
                    } else {
                        return Err(RpcError::new("Error", "Attachment content is unavailable"));
                    }
                }
            },
        };
        if data.len() > MAX_ATTACHMENT_BYTES {
            return Err(RpcError::new(
                "Error",
                "Attachment exceeds protocol size limit",
            ));
        }
        let upload_id = uuid::Uuid::new_v4().to_string();
        let total_chunks = data.len().div_ceil(CHUNK_BYTES);
        let checksum = format!(
            "sha256:{}",
            Sha256::digest(&data)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let file_name = attachment["filename"].as_str().unwrap_or_default();
        let mime = attachment["mimeType"].as_str().unwrap_or_default();
        let session_id = target["sessionId"].as_str().unwrap_or_default();
        let common = json!({
            "connectionId": connection_id,
            "sessionId": session_id,
            "uploadId": upload_id,
        });
        let mut begin = common.clone();
        begin["fileName"] = Value::from(file_name);
        begin["mime"] = Value::from(mime);
        begin["totalBytes"] = Value::from(data.len());
        begin["totalChunks"] = Value::from(total_chunks);
        begin["checksum"] = Value::from(checksum);
        let result = call(base, target, "v4/attachment/begin", begin).await;
        let begin = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = call(base, target, "v4/attachment/abort", common.clone()).await;
                return Err(error);
            }
        };
        if begin["state"] == "committed" {
            refs.push(to_ref(attachment, &begin["ref"], data.len()));
            continue;
        }
        let mut next = begin["nextChunkIndex"].as_u64().unwrap_or(0) as usize;
        if next > total_chunks {
            let _ = call(base, target, "v4/attachment/abort", common.clone()).await;
            return Err(RpcError::new("Error", "Invalid attachment upload progress"));
        }
        while next < total_chunks {
            let start = next * CHUNK_BYTES;
            let end = (start + CHUNK_BYTES).min(data.len());
            let mut chunk = common.clone();
            chunk["chunkIndex"] = Value::from(next);
            chunk["dataBase64"] =
                Value::from(base64::engine::general_purpose::STANDARD.encode(&data[start..end]));
            match call(base, target, "v4/attachment/chunk", chunk).await {
                Ok(result) if result["nextChunkIndex"].as_u64() == Some((next + 1) as u64) => {
                    next += 1;
                }
                Ok(_) => {
                    let _ = call(base, target, "v4/attachment/abort", common.clone()).await;
                    return Err(RpcError::new("Error", "Invalid attachment upload progress"));
                }
                Err(error) => {
                    let _ = call(base, target, "v4/attachment/abort", common.clone()).await;
                    return Err(error);
                }
            }
        }
        match call(base, target, "v4/attachment/commit", common.clone()).await {
            Ok(committed) => refs.push(to_ref(attachment, &committed["ref"], data.len())),
            Err(error) => {
                let _ = call(base, target, "v4/attachment/abort", common).await;
                return Err(error);
            }
        }
    }
    Ok(refs)
}

fn to_ref(attachment: &Value, reference: &Value, bytes: usize) -> Value {
    let mut value = json!({
        "ref": reference,
        "fileName": attachment["filename"],
        "mime": attachment["mimeType"],
        "bytes": bytes,
    });
    if attachment["kind"] == "file" {
        if let Some(kind) = attachment["sourceKind"].as_str() {
            value["sourceKind"] = Value::from(kind);
        }
        if let Some(count) = attachment["messageCount"].as_u64() {
            value["messageCount"] = Value::from(count);
        }
    }
    value
}

async fn call(
    base: &Arc<Base>,
    target: &Value,
    method: &str,
    body: Value,
) -> Result<Value, RpcError> {
    base.call(target, RuntimePolicy::StartIfNeeded, method, body)
        .await
        .map(|called| called.result)
}
