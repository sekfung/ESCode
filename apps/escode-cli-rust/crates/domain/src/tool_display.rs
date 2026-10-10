//! 工具卡展示载荷（docs/specs/rust-browser-use.md 第 3 期），对齐 TS `result-display.ts`：
//! MCP 工具卡（`mcp_tool`）、node_repl 图片（`node_repl_images`）与行级 display 白名单（`toProtocolToolCallDisplay`）。
use serde_json::{Value, json};

const MAX_NAME_CHARS: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 4 * 1024;
pub const MAX_NODE_REPL_IMAGE_BASE64_BYTES: usize = 200 * 1024;
const MAX_NODE_REPL_IMAGES: usize = 2;

/// TS `boundMcpDisplayText`：trim 后按 UTF-16 长度截断，不留半个代理对；空串视为缺失。
fn bound(value: &str, max_utf16: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut used = 0;
    let mut out = String::new();
    for c in trimmed.chars() {
        used += c.len_utf16();
        if used > max_utf16 {
            break;
        }
        out.push(c);
    }
    Some(out)
}

/// TS `createMcpToolDisplay`（非官方 MCP，无 unavailable 标识）。
pub fn mcp_tool(server: &str, tool: &str, description: Option<&str>) -> Option<Value> {
    let (server, tool) = (bound(server, MAX_NAME_CHARS)?, bound(tool, MAX_NAME_CHARS)?);
    let mut display = json!({"kind": "mcp_tool", "serverName": server, "toolName": tool});
    if let Some(description) = description.and_then(|d| bound(d, MAX_DESCRIPTION_CHARS)) {
        display["description"] = description.into();
    }
    Some(display)
}

/// TS `createNodeReplDisplay`：取 `images` 与 `content` 中的图片（每张 ≤200 KiB base64，最多两张），无图不产出。
pub fn node_repl_images(result: &Value) -> Option<Value> {
    let candidates = [&result["images"], &result["content"]]
        .into_iter()
        .filter_map(Value::as_array)
        .flatten();
    let (mut images, mut truncated) = (Vec::new(), false);
    for candidate in candidates {
        let Some(mime) = candidate["mimeType"].as_str().filter(|m| is_image_mime(m)) else { continue };
        let Some(encoded) = candidate["base64"].as_str().or_else(|| candidate["data"].as_str()) else { continue };
        let base64 = match encoded.strip_prefix("data:") {
            Some(rest) => rest.split_once(',').map_or("", |(_, b)| b),
            None => encoded,
        };
        if base64.is_empty() || base64.len() > MAX_NODE_REPL_IMAGE_BASE64_BYTES || images.len() >= MAX_NODE_REPL_IMAGES {
            truncated = true;
            continue;
        }
        images.push(json!({"base64": base64, "mimeType": mime}));
    }
    if images.is_empty() {
        return None;
    }
    let mut display = json!({"kind": "node_repl_images", "images": images});
    if truncated {
        display["truncated"] = true.into();
    }
    Some(display)
}

fn is_image_mime(mime: &str) -> bool {
    mime.len() > 6
        && mime[..6].eq_ignore_ascii_case("image/")
        && mime[6..].chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c))
}

/// TS `toProtocolToolCallDisplay`：只有这些 kind 投影到行级 `display`。
pub fn row_display_kind(display: &Value) -> bool {
    matches!(
        display["kind"].as_str(),
        Some(
            "node_repl_images"
                | "task_output"
                | "respond_to_coordinator"
                | "mcp_tool"
                | "create_workflow"
                | "get_workflow_run"
                | "list_workflow_runs"
                | "eval_workflow_snippet"
                | "saved_workflow_list"
                | "list_models"
                | "resume_workflow_run"
        )
    )
}

/// TS `fitStringToBytes(_, max, "head")`：保留不超过 `max` 字节的合法 UTF-8 前缀。
fn fit_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// TS resultBudget（`strategy: "truncate"`、`preview.direction: "head"`）：超预算时保留头部并附同一条
/// 说明后缀（`fitContentWithSuffix`）。`originalBytes` 报的是**截断前**的字节数。
pub fn truncate_model_content(text: String, max_model_bytes: usize) -> String {
    if max_model_bytes == 0 {
        return String::new();
    }
    if text.len() <= max_model_bytes {
        return text;
    }
    let suffix = format!(
        "\n\n[Tool output truncated by resultBudget: originalBytes={}, maxModelBytes={max_model_bytes}, strategy=truncate]",
        text.len()
    );
    let suffix = fit_prefix(&suffix, max_model_bytes);
    let mut out = fit_prefix(&text, max_model_bytes.saturating_sub(suffix.len()));
    out.push_str(&suffix);
    out
}

/// TS `boundDisplayText`：超过字节上限时保留前缀并附 `\n...[truncated]`（display 通道没有工具预算兜着）。
pub fn bound_display_text(value: &str, max_bytes: usize) -> (String, bool) {
    const SUFFIX: &str = "\n...[truncated]";
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let suffix = fit_prefix(SUFFIX, max_bytes);
    let mut out = fit_prefix(value, max_bytes.saturating_sub(suffix.len()));
    out.push_str(&suffix);
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_display_bounds_and_omits_empty_description() {
        assert_eq!(
            mcp_tool(" node_repl ", "js", Some("  ")),
            Some(json!({"kind":"mcp_tool","serverName":"node_repl","toolName":"js"}))
        );
        assert_eq!(mcp_tool("", "js", None), None);
        let long = "😀".repeat(3000);
        let bounded = mcp_tool("s", "t", Some(&long)).unwrap()["description"].as_str().unwrap().to_owned();
        assert_eq!(bounded.encode_utf16().count(), MAX_DESCRIPTION_CHARS);
    }

    #[test]
    fn node_repl_images_follow_ts_limits() {
        let img = |data: &str| json!({"type":"image","mimeType":"image/png","data":data});
        let result = json!({"content":[{"type":"text","text":"x"}, img("AAAA"), img("data:image/png;base64,BBBB"), img("CCCC")]});
        assert_eq!(
            node_repl_images(&result),
            Some(json!({"kind":"node_repl_images","images":[{"base64":"AAAA","mimeType":"image/png"},{"base64":"BBBB","mimeType":"image/png"}],"truncated":true}))
        );
        assert_eq!(node_repl_images(&json!({"content":[{"type":"text","text":"x"}]})), None);
        assert!(row_display_kind(&json!({"kind":"mcp_tool"})));
        assert!(!row_display_kind(&json!({"kind":"bash"})));
    }
}
