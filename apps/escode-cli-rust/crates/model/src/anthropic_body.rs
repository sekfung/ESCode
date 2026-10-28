//! Anthropic Messages 请求体：system 块、thinking 回放、tool_use/tool_result 与缓存断点。
use super::{config::ModelConfig, contract::ModelFailure};
use serde_json::{Value, json};

/// Anthropic 规范要求 `thinking.type == "enabled"` 必带 `budget_tokens`（1024 ≤ budget < max_tokens）。
/// 选项映射只输出档位语义且无法引用 maxOutputTokens，因此在最终正文上补齐；依据
/// [Anthropic Messages Thinking Budget Protocol](../../../../../docs/anthropic-thinking-budget-protocol.md)。
pub(super) fn ensure_thinking_budget(body: &mut Value) {
    if body["thinking"]["type"] != "enabled" {
        return;
    }
    if body["thinking"]["budget_tokens"]
        .as_u64()
        .is_some_and(|budget| budget > 0)
    {
        return;
    }
    let Some(max_tokens) = body["max_tokens"].as_u64().filter(|m| *m > 0) else {
        return;
    };
    let budget = (max_tokens / 2).max(1024).min(max_tokens - 1);
    if budget < 1024 {
        // max_tokens 撑不起规范下限：省略 thinking（不启用思考），不发必然被拒的正文。
        body.as_object_mut().unwrap().remove("thinking");
        return;
    }
    body["thinking"]["budget_tokens"] = budget.into();
}

pub(super) fn body(
    config: &ModelConfig,
    messages: &[Value],
    tools: &[Value],
) -> Result<Value, ModelFailure> {
    let mut system = vec![];
    let cache_system = messages
        .iter()
        .any(|m| m["role"] == "system" && m["_escode_cache_control"].is_object());
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
            if message["_escode_cache_control"].is_object() {
                block["cache_control"] = message["_escode_cache_control"].clone();
            }
            system.push(block);
            continue;
        }
        if let Some(blocks) = message["_escode_anthropic_thinking"].as_array() {
            content.extend_from_slice(blocks);
        }
        if role == "tool" {
            // 工具结果 content 可能是 part 数组（媒体、native tool_reference）；按 Anthropic 格式投影，
            // 不能把内部 camelCase part 原样发出。
            let tool_content = tool_result_content(&message["content"])?;
            let mut result = json!({"type":"tool_result","tool_use_id":message["tool_call_id"],"content":tool_content});
            // AI SDK 只在 error 输出时写 is_error。
            if message["_escode_tool_failed"] == true {
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
        if message["_escode_synthetic"] != true {
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
            None => {
                let mut tool = json!({"name":t["function"]["name"],"description":t["function"]["description"],"input_schema":t["function"]["parameters"]});
                // 上游 provider 把内部 camelCase 的 deferLoading 映射为 Anthropic 的 defer_loading。
                if t["function"]["deferLoading"] == true {
                    tool["defer_loading"] = true.into();
                }
                tool
            }
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

fn tool_result_content(content: &Value) -> Result<Value, ModelFailure> {
    if content.is_array() {
        Ok(Value::Array(super::model_media::anthropic(content)?))
    } else {
        Ok(content.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_result_projects_tool_reference_parts() {
        let content = json!([
            {"type":"text","text":"loaded"},
            {"type":"tool_reference","toolName":"mcp__a__b"}
        ]);
        assert_eq!(
            tool_result_content(&content).unwrap(),
            json!([
                {"type":"text","text":"loaded"},
                {"type":"tool_reference","tool_name":"mcp__a__b"}
            ])
        );
    }

    #[test]
    fn tool_result_keeps_plain_strings() {
        assert_eq!(tool_result_content(&json!("done")).unwrap(), json!("done"));
    }
}
