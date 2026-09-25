//! MCP `tools/call` 结果的模型内容（docs/specs/rust-mcp-parity.md 第 1、2 期），逐条对齐 TS
//! `core/src/mcp/index.ts` `formatMcpToolResult` 与 `image-normalization.ts` 的 inline 预算。
//! 缩进 JSON 优先按保序解析输出（rmcp 类型字段顺序）；`structuredContent` 在 rmcp 中为无序 Value，
//! 其键序按字母排序，可能与 server 原序不同（已知差异）。
use super::json_order::Json;
use serde_json::{Value, json};

pub const IMAGE_INLINE_BASE64_BYTES: usize = 200 * 1024;
const ERROR_PRESENTATION_KEY: &str = "zcode/errorPresentation";

enum Block {
    Text(String),
    Image { mime: String, url: String },
}

/// 结果文本与媒体 part（chat 形态，含 `_zcode_name`）；无媒体时 `media` 为空。
pub struct Formatted {
    pub text: String,
    pub media: Vec<Value>,
}

/// JS `JSON.stringify(value, null, 2)`；有保序解析时按原键序输出。
fn pretty(value: &Value, ordered: Option<&Json>) -> String {
    match ordered {
        Some(json) => json.pretty(),
        None => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}
/// TS formatByteSize（B/KiB/MiB/GiB；≥10 或字节单位取整，否则一位小数）。
pub fn byte_size(bytes: usize) -> String {
    if bytes == 0 {
        return "0 B".into();
    }
    let units = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let formatted = if value >= 10.0 || unit == 0 {
        format!("{}", (value + 0.5).floor())
    } else {
        format!("{value:.1}")
    };
    format!("{formatted} {}", units[unit])
}
fn payload(data: &str) -> &str {
    match data.strip_prefix("data:") {
        Some(_) => data.find(',').map_or(data, |i| &data[i + 1..]),
        None => data,
    }
}
fn image_block(block: &Value) -> Block {
    let mime = block["mimeType"].as_str();
    let (Some(data), Some(mime)) = (block["data"].as_str(), mime) else {
        return Block::Text(format!(
            "[MCP image content omitted: {}]",
            mime.unwrap_or("unknown")
        ));
    };
    let base64 = payload(data).len();
    if base64 > IMAGE_INLINE_BASE64_BYTES {
        // TS 无 artifact store 时的省略文案（Rust 暂不落 MCP 图片 artifact）。
        return Block::Text(format!(
            "MCP image content omitted: {mime}, base64={} exceeds inline limit {}.\nNo artifact store is configured, so the original image could not be saved.",
            byte_size(base64),
            byte_size(IMAGE_INLINE_BASE64_BYTES)
        ));
    }
    let url = if data.starts_with("data:") {
        data.to_owned()
    } else {
        format!("data:{mime};base64,{data}")
    };
    Block::Image {
        mime: mime.to_owned(),
        url,
    }
}
fn content_block(block: &Value, ordered: Option<&Json>) -> Option<Block> {
    match block["type"].as_str() {
        Some("text") if block["text"].is_string() => {
            let text = block["text"].as_str().unwrap_or_default();
            (!text.is_empty()).then(|| Block::Text(text.to_owned()))
        }
        Some("image") => Some(image_block(block)),
        Some("audio") => Some(Block::Text(format!(
            "[MCP audio content omitted: {}]",
            block["mimeType"].as_str().unwrap_or("unknown")
        ))),
        Some("resource") => {
            let (resource, ordered) = if block["resource"].is_null() {
                (block, ordered)
            } else {
                (&block["resource"], ordered.and_then(|o| o.get("resource")))
            };
            Some(Block::Text(format!(
                "MCP resource content:\n{}",
                pretty(resource, ordered)
            )))
        }
        _ => Some(Block::Text(pretty(block, ordered))),
    }
}
fn informative(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        _ => true,
    }
}
fn textify(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text(text) => text.clone(),
            Block::Image { mime, .. } => format!("[Attached {mime}: MCP image]"),
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn format(result: &Value, ordered: Option<&Json>) -> Formatted {
    let text_only = |text: String| Formatted {
        text,
        media: vec![],
    };
    let Some(content) = result["content"].as_array() else {
        return text_only(pretty(result, ordered));
    };
    let ordered_content = ordered
        .and_then(|o| o.get("content"))
        .and_then(Json::as_array);
    let mut blocks: Vec<Block> = content
        .iter()
        .enumerate()
        .filter_map(|(i, block)| content_block(block, ordered_content.and_then(|c| c.get(i))))
        .collect();
    if let Some(structured) = result.get("structuredContent").filter(|v| informative(v)) {
        let ordered = ordered.and_then(|o| o.get("structuredContent"));
        blocks.push(Block::Text(format!(
            "Structured content:\n{}",
            pretty(structured, ordered)
        )));
    }
    if blocks.is_empty() {
        return text_only(pretty(result, ordered));
    }
    let message_only = result["_meta"][ERROR_PRESENTATION_KEY] == "message-only";
    if result["isError"] == true && !message_only {
        return text_only(format!("MCP tool returned an error:\n{}", textify(&blocks)));
    }
    let text = textify(&blocks);
    if blocks.iter().all(|b| matches!(b, Block::Text(_))) {
        return text_only(text);
    }
    let media = blocks
        .into_iter()
        .map(|b| match b {
            Block::Text(text) => json!({"type":"text","text":text}),
            Block::Image { url, .. } => {
                json!({"type":"image_url","image_url":{"url":url},"_zcode_name":"MCP image"})
            }
        })
        .collect();
    Formatted { text, media }
}
