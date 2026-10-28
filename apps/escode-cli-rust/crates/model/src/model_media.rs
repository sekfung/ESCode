use crate::contract::ModelFailure;
use serde_json::{Value, json};
type Result<T> = std::result::Result<T, ModelFailure>;
pub(super) fn responses(content: &Value) -> Result<Vec<Value>> {
    content.as_array().ok_or_else(ModelFailure::invalid)?.iter().map(|p|Ok(match p["type"].as_str(){
        // EasyInputMessage 的 content 列表只接受 input_* 类型，`output_text` 只合法于带 id/status 的
        // ResponseOutputMessage item；旧写法按 role 给 assistant 发 output_text 会被严格实现（vLLM）拒绝。
        // 依据：docs/openai-responses-input-item-protocol.md。
        Some("text")=>json!({"type":"input_text","text":p["text"]}),
        Some("image_url")=>json!({"type":"input_image","image_url":p["image_url"]["url"]}),
        Some("video_url")=>return Err(ModelFailure::new("attachment_unsupported", false)),
        Some("file")=>{let data=p["file"]["file_data"].as_str().ok_or_else(ModelFailure::invalid)?;if data.starts_with("data:"){json!({"type":"input_file","filename":p["file"]["filename"],"file_data":data})}else{json!({"type":"input_file","file_url":data})}},
        _=>return Err(ModelFailure::invalid()),
    })).collect()
}
pub(super) fn anthropic(content: &Value) -> Result<Vec<Value>> {
    content.as_array().ok_or_else(ModelFailure::invalid)?.iter().map(|p|Ok(match p["type"].as_str(){
        Some("text")=>p.clone(),
        Some("image_url")=>json!({"type":"image","source":source(p["image_url"]["url"].as_str().ok_or_else(ModelFailure::invalid)?)?}),
        Some("video_url")=>json!({"type":"video","source":source(p["video_url"]["url"].as_str().ok_or_else(ModelFailure::invalid)?)?}),
        Some("file")=>json!({"type":"document","source":source(p["file"]["file_data"].as_str().ok_or_else(ModelFailure::invalid)?)?}),
        // ToolSearch native：工具结果里的引用块由 provider 展开 schema（docs/specs/rust-tool-search.md）。
        Some("tool_reference")=>json!({"type":"tool_reference","tool_name":p["toolName"].as_str().ok_or_else(ModelFailure::invalid)?}),
        _=>return Err(ModelFailure::invalid()),
    })).collect()
}
fn source(url: &str) -> Result<Value> {
    if let Some(data) = url.strip_prefix("data:") {
        let (mime, bytes) = data
            .split_once(";base64,")
            .ok_or_else(ModelFailure::invalid)?;
        Ok(json!({"type":"base64","media_type":mime,"data":bytes}))
    } else if url.starts_with("https://") || url.starts_with("http://") {
        Ok(json!({"type":"url","url":url}))
    } else {
        Err(ModelFailure::invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_maps_tool_reference_to_wire_shape() {
        let content = json!([{"type":"tool_reference","toolName":"mcp__a__b"}]);
        assert_eq!(
            anthropic(&content).unwrap(),
            vec![json!({"type":"tool_reference","tool_name":"mcp__a__b"})]
        );
    }

    #[test]
    fn anthropic_rejects_unknown_part() {
        assert!(anthropic(&json!([{"type":"unknown"}])).is_err());
    }
}
