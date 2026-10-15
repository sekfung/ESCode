//! `prompt-attachment-transfer`（本地实现）与 `media-preview.prepare`（内联分支）
//! （docs/specs/rust-u2-attachments.md）。

use std::sync::Arc;

use futures_util::stream;
use serde_json::{Value, json};

use crate::channel::{Channel, EventHandler, EventStream};
use crate::domains_file::stat;
use crate::domains_file_preview::{
    MAX_MEDIA_PREVIEW_BYTES, MEDIA_PREVIEW_FORMATS, read_media_preview,
};
use crate::session::{RpcError, handler, handler_rpc};
use crate::wire::Wire;

fn params_of(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

/// TS `createLocalPromptAttachmentTransferService`：零拷贝，不伪造上传进度。
pub(crate) fn prompt_attachment_transfer_channel() -> Channel {
    let stage = handler(move |arg: Wire| {
        Box::pin(async move {
            let params = params_of(arg);
            let local_path = params["localPath"].as_str().unwrap_or_default().to_owned();
            let declared = params["sizeBytes"].as_f64().filter(|size| *size > 0.0);
            let bytes = match declared {
                Some(size) => size,
                None => tokio::fs::metadata(&local_path)
                    .await
                    .map_or(0.0, |meta| meta.len() as f64),
            };
            Ok(Wire::Object(json!({
                "operationId": params["operationId"],
                "ref": local_path,
                "bytes": bytes,
                "staged": false,
            })))
        })
    });
    let noop = || handler(|_arg: Wire| Box::pin(async move { Ok(Wire::Undefined) }));
    // 订阅有效、永不触发：Renderer 按 operationId 订阅进度，本地实现没有进度可报。
    let progress: EventHandler = Arc::new(|_arg: Wire| {
        let stream: EventStream = Box::pin(stream::pending());
        Ok(stream)
    });
    Channel::new(
        "prompt-attachment-transfer",
        vec![
            ("stage", stage),
            ("adopt", noop()),
            ("cancel", noop()),
            ("cleanup", noop()),
        ],
    )
    .with_events(vec![("onDynamicProgress", progress)])
}

/// TS `getMediaPreviewFormat`：`\`→`/` 并小写后按表序 `endsWith`。
fn media_format(path: &str) -> Option<(&'static str, &'static str)> {
    let normalized = path.replace('\\', "/").to_lowercase();
    MEDIA_PREVIEW_FORMATS
        .iter()
        .find(|(extension, _)| normalized.ends_with(extension))
        .map(|(_, media_type)| {
            (
                *media_type,
                media_type.split('/').next().unwrap_or_default(),
            )
        })
}

fn plain_error(message: String) -> RpcError {
    RpcError::new("Error", message)
}

fn prepare(params: &Value) -> Result<Wire, RpcError> {
    let path = params["path"].as_str().unwrap_or_default();
    let expected = params["expectedKind"].as_str().unwrap_or_default();
    match media_format(path) {
        Some((_, kind)) if kind == expected => {}
        _ => {
            return Err(plain_error(format!(
                "Unsupported media preview format: {path}"
            )));
        }
    }
    let stat_result =
        stat(&json!({"path": path})).map_err(<RpcError as From<(String, String)>>::from)?;
    let Wire::Object(stat_value) = stat_result else {
        return Err(plain_error(format!("Path is not a media file: {path}")));
    };
    let size = stat_value["size"].as_u64();
    let Some(size) = size.filter(|_| stat_value["type"] == "file") else {
        return Err(plain_error(format!("Path is not a media file: {path}")));
    };
    if size > MAX_MEDIA_PREVIEW_BYTES {
        return Err(plain_error(format!(
            "Media file is too large for inline preview: {path}"
        )));
    }
    let preview = read_media_preview(&json!({"path": path, "maxBytes": MAX_MEDIA_PREVIEW_BYTES}))
        .map_err(<RpcError as From<(String, String)>>::from)?;
    let Wire::Object(preview) = preview else {
        return Err(plain_error(format!("Path is not a media file: {path}")));
    };
    Ok(Wire::Object(json!({
        "kind": "inline",
        "dataBase64": preview["dataBase64"],
        "mediaType": preview["mediaType"],
        "path": path,
        "size": preview["totalBytes"],
    })))
}

pub(crate) fn media_preview_channel() -> Channel {
    Channel::new(
        "media-preview",
        vec![(
            "prepare",
            handler_rpc(|arg: Wire| {
                Box::pin(async move {
                    let params = params_of(arg);
                    tokio::task::spawn_blocking(move || prepare(&params))
                        .await
                        .map_err(|error| RpcError::new("Fatal", error.to_string()))?
                })
            }),
        )],
    )
}

#[cfg(test)]
#[path = "domains_attachments_tests.rs"]
mod tests;
