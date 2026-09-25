use crate::{contract::ModelFailure, domain::session::StoredAttachment};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

type Result<T> = std::result::Result<T, ModelFailure>;
pub(super) async fn materialize(messages: &mut Vec<Value>, properties: &Value) -> Result<bool> {
    let mut expanded = false;
    let mut total = 0u64;
    for message in messages.iter_mut() {
        let tool = message["role"] == "tool";
        let Some(parts) = message["content"].as_array_mut() else {
            continue;
        };
        // TS projectMessagesWithMediaAttachmentPaths：用户本地文件媒体在消息末尾附 `[Image: source: <path>]`。
        let mut sources = vec![];
        for part in parts.iter_mut() {
            if part["type"] != "_zcode_attachment" {
                continue;
            }
            expanded = true;
            let asset: StoredAttachment = serde_json::from_value(part["asset"].clone())
                .map_err(|_| ModelFailure::new("attachment_unavailable", false))?;
            total = total.saturating_add(asset.total_bytes);
            if asset.total_bytes > 20 * 1024 * 1024 || total > 64 * 1024 * 1024 {
                return Err(ModelFailure::new("context_exceeded", false));
            }
            let mime = asset.media_type.as_str();
            let capability = if mime.starts_with("image/") {
                Some("supportsImage")
            } else if mime == "application/pdf" {
                Some("supportsPdf")
            } else if mime.starts_with("video/") {
                Some("supportsVideo")
            } else {
                None
            };
            if let Some(key) = capability.filter(|key| properties["inputFormat"][key] != true) {
                // 工具结果媒体在模型不支持时以占位文本交付（TS createUnsupportedModelInputMediaText），
                // 用户附件仍拒绝请求。
                if !tool {
                    return Err(ModelFailure::new("attachment_unsupported", false));
                }
                let kind = match key {
                    "supportsImage" => "image input",
                    "supportsPdf" => "PDF input",
                    _ => "video input",
                };
                let name = part["name"].as_str().unwrap_or_default();
                let placeholder = if name.is_empty() {
                    format!("[Attached {mime}]")
                } else {
                    format!("[Attached {mime}: {name}]")
                };
                *part = json!({"type":"text","text":format!("{placeholder}\n[Media omitted from provider request because the selected model does not support {kind}.]")});
                continue;
            }
            let mut file = tokio::fs::File::open(&asset.path)
                .await
                .map_err(|_| ModelFailure::new("attachment_unavailable", false))?;
            if file
                .metadata()
                .await
                .map_err(|_| ModelFailure::new("attachment_unavailable", false))?
                .len()
                != asset.total_bytes
            {
                return Err(ModelFailure::new("attachment_unavailable", false));
            }
            let mut bytes = vec![];
            (&mut file)
                .take(asset.total_bytes + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| ModelFailure::new("attachment_unavailable", false))?;
            if bytes.len() as u64 != asset.total_bytes {
                return Err(ModelFailure::new("attachment_unavailable", false));
            }
            let name = part["name"].as_str().unwrap_or("attachment");
            // TS projectMessagesWithMediaAttachmentPaths：本地文件用其路径；上传附件派生一份本地副本
            // （原始字节、原 MIME 扩展名）。派生失败与 TS 一样使请求失败，不静默丢掉路径。
            let source = if tool || capability.is_none() {
                None
            } else if let Some(path) = asset.source_path.as_deref() {
                Some(path.to_owned())
            } else if let Some(uri) = part["ref"].as_str().filter(|r| r.starts_with("zcode-artifact://")) {
                super::attachment_image::derived_media_path(uri, mime, &bytes)
                    .await
                    .map_err(|_| ModelFailure::new("attachment_unavailable", false))?
            } else {
                None
            };
            // 工具结果媒体已由工具按同一预算准备；用户图片在此准备（TS prepareImageDataUrl）。
            let prepared;
            let (mime, bytes) = if mime.starts_with("image/") && !tool {
                let key = format!("{}:{}", asset.path, asset.total_bytes);
                prepared = super::attachment_image::prepare(key, bytes, mime).await;
                match prepared.as_ref() {
                    Some(prepared) => (prepared.media_type.as_str(), prepared.data.clone()),
                    None => {
                        // TS resolvedPlaceholderAttachment：无法解码/压缩时以占位文本交付，不中断请求。
                        let label = asset.source_path.as_deref().unwrap_or(name);
                        *part = json!({"type":"text","text":format!("[Attached {mime}: {label}]")});
                        continue;
                    }
                }
            } else {
                (mime, bytes)
            };
            if let Some(path) = source {
                let label = match capability {
                    Some("supportsImage") => "Image",
                    Some("supportsVideo") => "Video",
                    _ => "PDF",
                };
                sources.push(format!("[{label}: source: {path}]"));
            }
            *part = if capability.is_some() {
                if mime == "application/pdf" && !bytes.starts_with(b"%PDF-") {
                    return Err(ModelFailure::new("attachment_unavailable", false));
                }
                let data = format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                );
                // `_zcode_name` 供工具结果文本化占位使用，协议层发送前剥离。
                if mime.starts_with("image/") {
                    json!({"type":"image_url","image_url":{"url":data},"_zcode_name":name})
                } else if mime.starts_with("video/") {
                    json!({"type":"video_url","video_url":{"url":data},"_zcode_name":name})
                } else {
                    json!({"type":"file","file":{"filename":name,"file_data":data},"_zcode_name":name})
                }
            } else {
                let name = asset.source_path.as_deref().unwrap_or(name);
                let text = std::str::from_utf8(&bytes)
                    .ok()
                    .filter(|s| !s.contains('\0'));
                // 修复：原先把文本附件拼进用户消息（自拟文案、64 KiB 截断）；TS 以一次 Read 调用
                // 结果的 system-reminder 独立成条放在用户正文之前（见 attachment_reminder.rs）。
                // TS 先按扩展名判定（isTextLikePath），非文本扩展名只交付路径引用、不读正文。
                if !super::attachment_read::is_text_like_path(name) {
                    json!({"type":"text","text":super::attachment_read::path_reference(name)})
                } else {
                    match text {
                        Some(text) => {
                            let text = text.strip_prefix('\u{feff}').unwrap_or(text);
                            json!({"type":"_zcode_reminder","message":super::attachment_reminder::reminder_message(name, text, bytes.len())})
                        }
                        None => json!({"type":"text","text":format!(
                            "Attached binary file: {name} ({mime}, {} bytes). The contents are not text and have not been included in this model request.",
                            asset.total_bytes
                        )}),
                    }
                }
            };
        }
        parts.extend(sources.into_iter().map(|text| json!({"type":"text","text":text})));
    }
    if expanded {
        hoist_reminders(messages);
    }
    Ok(expanded)
}

/// 把附件 reminder 移到所属 user 消息之前；剩余正文只有一段文本时收成字符串（TS normalizeRealUserContent）。
fn hoist_reminders(messages: &mut Vec<Value>) {
    let mut out = Vec::with_capacity(messages.len());
    for mut message in std::mem::take(messages) {
        if let Some(parts) = message["content"].as_array_mut()
            && parts.iter().any(|p| p["type"] == "_zcode_reminder")
        {
            let (reminders, rest): (Vec<Value>, Vec<Value>) = std::mem::take(parts)
                .into_iter()
                .partition(|p| p["type"] == "_zcode_reminder");
            out.extend(reminders.into_iter().map(|r| r["message"].clone()));
            message["content"] = match rest.as_slice() {
                [] => Value::String(String::new()),
                [only] if only["type"] == "text" => only["text"].clone(),
                _ => Value::Array(rest),
            };
        }
        out.push(message);
    }
    *messages = out;
}
