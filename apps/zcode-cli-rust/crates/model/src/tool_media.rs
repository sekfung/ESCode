//! 工具结果媒体按 API 格式投影（TS `transform.ts` + `tool-result-media-projection.ts`），
//! 见 docs/specs/rust-media-read.md：
//! - Chat Completions：tool 消息只放文本化结果；同一段工具结果之后追加一条 user 消息
//!   （`Tool result media from <tool>:` + 媒体 part）。
//! - Anthropic：tool_result 内嵌媒体块；Responses：function_call_output 结构化输出。
//! - 工具报错只给文本；模型不支持的媒体替换为占位说明文本。
use crate::{contract::ModelFailure, model_protocol::ApiType};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, ModelFailure>;

fn media_url(part: &Value) -> Option<&str> {
    part["image_url"]["url"]
        .as_str()
        .or_else(|| part["video_url"]["url"].as_str())
        .or_else(|| part["file"]["file_data"].as_str())
}
fn media_mime(part: &Value) -> Option<String> {
    let rest = media_url(part)?.strip_prefix("data:")?;
    let header = &rest[..rest.find(',')?];
    Some(header.split(';').next()?.trim().to_lowercase())
}
/// TS attachmentPlaceholder("Attached", mediaType, name)。
fn placeholder(part: &Value) -> String {
    let mime = media_mime(part).unwrap_or_default();
    match part["_zcode_name"].as_str().filter(|n| !n.is_empty()) {
        Some(name) => format!("[Attached {mime}: {name}]"),
        None => format!("[Attached {mime}]"),
    }
}
/// TS modelMessageContentToText。
fn textify(parts: &[Value]) -> String {
    parts
        .iter()
        .map(|p| match p["type"].as_str() {
            Some("text") => p["text"].as_str().unwrap_or_default().to_owned(),
            _ => placeholder(p),
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}
/// 模型不支持的媒体替换为 TS createUnsupportedModelInputMediaText 的文本。
fn unsupported(part: &Value, properties: &Value) -> Option<String> {
    let (key, kind) = match part["type"].as_str()? {
        "image_url" => ("supportsImage", "image input"),
        "video_url" => ("supportsVideo", "video input"),
        "file" if media_mime(part).as_deref() == Some("application/pdf") => {
            ("supportsPdf", "PDF input")
        }
        _ => return None,
    };
    (properties["inputFormat"][key] != true).then(|| {
        format!(
            "{}\n[Media omitted from provider request because the selected model does not support {kind}.]",
            placeholder(part)
        )
    })
}
fn strip(parts: &mut [Value]) {
    for part in parts {
        if let Some(object) = part.as_object_mut() {
            object.remove("_zcode_name");
        }
    }
}

pub(super) fn project(
    messages: Vec<Value>,
    api: ApiType,
    properties: &Value,
) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(messages.len());
    let mut pending: Vec<Value> = vec![];
    for mut message in messages {
        let tool = message["role"] == "tool";
        if !tool && !pending.is_empty() {
            out.push(json!({"role":"user","content":std::mem::take(&mut pending),"_zcode_synthetic":true}));
        }
        if tool && let Some(parts) = message["content"].as_array().cloned() {
            let mut parts: Vec<Value> = parts
                .into_iter()
                .map(|p| match unsupported(&p, properties) {
                    Some(text) => json!({"type":"text","text":text}),
                    None => p,
                })
                .collect();
            let failed = message["_zcode_tool_failed"] == true;
            // 含视频的结果在所有协议上都文本化并后置 user part（AI SDK tool result 无 video 变体）。
            let video = parts.iter().any(|p| p["type"] == "video_url");
            let name = message["_zcode_tool_name"]
                .as_str()
                .unwrap_or("tool")
                .to_owned();
            message["content"] = if failed {
                textify(&parts).into()
            } else if video || api == ApiType::Chat {
                let mut media: Vec<Value> = parts
                    .iter()
                    .filter(|p| p["type"] != "text")
                    .cloned()
                    .collect();
                if !media.is_empty() {
                    strip(&mut media);
                    pending.push(
                        json!({"type":"text","text":format!("Tool result media from {name}:")}),
                    );
                    pending.extend(media);
                }
                textify(&parts).into()
            } else if api == ApiType::Anthropic {
                strip(&mut parts);
                super::model_media::anthropic(&Value::Array(parts))?.into()
            } else {
                strip(&mut parts);
                super::model_media::responses(&Value::Array(parts), "user")?.into()
            };
        } else if let Some(parts) = message["content"].as_array_mut() {
            strip(parts);
        }
        out.push(message);
    }
    if !pending.is_empty() {
        out.push(json!({"role":"user","content":pending,"_zcode_synthetic":true}));
    }
    Ok(out)
}
