//! Anthropic Messages 请求体：system 块、thinking 回放、tool_use/tool_result 与缓存断点。
use super::{config::ModelConfig, contract::ModelFailure};
use serde_json::{Value, json};

pub(super) fn body(
    config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
) -> Result<Value, ModelFailure> {
    let mut system = vec![];
    let cache_system = messages
        .iter()
        .any(|m| m["role"] == "system" && m["_zcode_cache_control"].is_object());
    let mut output: Vec<Value> = vec![];
    // 缓存断点：最后一条非合成消息贡献的最后一个块（TS 在 provider 投影前标记，媒体后置消息不计）。
    let mut cache_target: Option<(usize, usize)> = None;
    for message in messages {
        let mut content = vec![];
        let role = message["role"].as_str().ok_or_else(ModelFailure::invalid)?;
        if role == "system" {
            let text = message["content"]
                .as_str()
                .ok_or_else(ModelFailure::invalid)?;
            let mut block = json!({"type":"text","text":text});
            if message["_zcode_cache_control"].is_object() {
                block["cache_control"] = message["_zcode_cache_control"].clone();
            }
            system.push(block);
            continue;
        }
        if let Some(blocks) = message["_zcode_anthropic_thinking"].as_array() {
            content.extend_from_slice(blocks);
        }
        if role == "tool" {
            let mut result = json!({"type":"tool_result","tool_use_id":message["tool_call_id"],"content":message["content"]});
            // AI SDK 只在 error 输出时写 is_error。
            if message["_zcode_tool_failed"] == true {
                result["is_error"] = true.into();
            }
            content.push(result);
        } else if message["content"].is_array() {
            content.extend(super::model_media::anthropic(&message["content"])?);
        } else if message["content"].as_str().is_some_and(|s| !s.is_empty()) {
            content.push(json!({"type":"text","text":message["content"]}));
        }
        if let Some(calls) = message["tool_calls"].as_array() {
            for call in calls {
                let input: Value = serde_json::from_str(
                    call["function"]["arguments"]
                        .as_str()
                        .ok_or_else(ModelFailure::invalid)?,
                )
                .map_err(|_| ModelFailure::invalid())?;
                if !input.is_object() {
                    return Err(ModelFailure::invalid());
                }
                content.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":input}));
            }
        }
        let role = if role == "tool" { "user" } else { role };
        // 对齐 TS reasoning-history-normalization；仅推理截断仍保留 canonical，请求不回放孤立 thinking。
        if content.is_empty()
            || (role == "assistant"
                && content
                    .iter()
                    .all(|b| matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking"))))
        {
            continue;
        }
        if let Some(last) = output.last_mut().filter(|m| m["role"] == role) {
            last["content"].as_array_mut().unwrap().extend(content);
        } else {
            let mut message = json!({"role":role});
            message["content"] = content.into();
            output.push(message);
        }
        if message["_zcode_synthetic"] != true {
            let index = output.len() - 1;
            cache_target = Some((
                index,
                output[index]["content"].as_array().unwrap().len() - 1,
            ));
        }
    }
    let tools = tools
        .iter()
        .map(|t| match super::web_search::anthropic_tool(t) {
            Some(native) => native,
            None => json!({"name":t["function"]["name"],"description":t["function"]["description"],"input_schema":t["function"]["parameters"]}),
        })
        .collect::<Vec<_>>();
    let mut body =
        json!({"model":config.model_id,"stream":true,"max_tokens":config.max_output_tokens});
    // 修复：非缓存请求（标题、WebFetch 处理、WebSearch、压缩摘要）此前把 system 拼成字符串，空时还会发 `""`；
    // AI SDK 始终发送文本块数组（每条 system 消息一块），没有 system 时省略该字段。
    if !system.is_empty() {
        body["system"] = system.into();
    }
    // TS finalizeLatestNonSystemMessageCacheControl：主请求把最新的非 system 消息设为缓存断点，
    // Anthropic 落在其最后一个内容块上（摘要等不带系统缓存前缀的请求不设）。
    if cache_system && let Some((message, block)) = cache_target {
        output[message]["content"][block]["cache_control"] = json!({"type":"ephemeral"});
    }
    body["messages"] = output.into();
    body["tools"] = tools.into();
    Ok(body)
}
