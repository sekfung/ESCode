//! 单次执行约束（协议 `modelExecutionSchema`，strict）。
//! 凭据只属于本次执行：调用方只在内存保存，不写入输入 payload、数据库、事件或日志。
//! 该作用域同时服务 highspeed 等按轮执行的模型选择与鉴权。

use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelExecution {
    /// `requestAuth {apiKey?, apiKeyId?, accountScope?, headers?}`：本轮鉴权材料。
    pub request_auth: Option<Value>,
    /// `selectionFallback {providerId, rules, target?}`：highspeed 失败时回到会话模型的声明。
    pub selection_fallback: Option<Value>,
}

fn strict(value: &Value, keys: &[&str]) -> Option<Map<String, Value>> {
    let object = value.as_object()?;
    object
        .keys()
        .all(|k| keys.contains(&k.as_str()))
        .then(|| object.clone())
}

fn non_empty(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.is_empty())
}

fn parse_request_auth(value: &Value) -> Option<Value> {
    let auth = strict(value, &["apiKey", "apiKeyId", "accountScope", "headers"])?;
    if auth.get("apiKey").is_some_and(|k| !non_empty(k))
        || auth.get("apiKeyId").is_some_and(|k| !non_empty(k))
        || auth.get("accountScope").is_some_and(|scope| {
            scope.as_str().is_none_or(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        })
    {
        return None;
    }
    if let Some(headers) = auth.get("headers") {
        let headers = headers.as_object()?;
        if headers
            .iter()
            .any(|(name, value)| name.is_empty() || !non_empty(value))
        {
            return None;
        }
    }
    Some(Value::Object(auth))
}

fn parse_selection_fallback(value: &Value) -> Option<Value> {
    let fallback = strict(value, &["providerId", "rules", "target"])?;
    if !non_empty(fallback.get("providerId")?) {
        return None;
    }
    let rules = fallback.get("rules")?.as_array()?;
    if rules.is_empty() {
        return None;
    }
    for rule in rules {
        let rule = strict(rule, &["reason", "providerErrorCode"])?;
        if !matches!(
            rule.get("reason").and_then(Value::as_str),
            Some("highspeed_card_expired" | "highspeed_request_failed")
        ) || rule
            .get("providerErrorCode")
            .is_some_and(|code| !non_empty(code))
        {
            return None;
        }
    }
    if let Some(target) = fallback.get("target") {
        let target = strict(target, &["providerId", "modelId", "options"])?;
        if !non_empty(target.get("providerId")?) || !non_empty(target.get("modelId")?) {
            return None;
        }
        if let Some(options) = target.get("options") {
            let options = strict(options, &["reasoningLevel"])?;
            if options
                .get("reasoningLevel")
                .is_some_and(|level| !non_empty(level))
            {
                return None;
            }
        }
    }
    Some(Value::Object(fallback))
}

pub fn parse(value: &Value) -> Result<ModelExecution, String> {
    let invalid = || "Invalid modelExecution".to_owned();
    let object = strict(
        value,
        &["selectionScope", "requestAuth", "selectionFallback"],
    )
    .ok_or_else(invalid)?;
    if object.get("selectionScope").and_then(Value::as_str) != Some("execution") {
        return Err(invalid());
    }
    let request_auth = match object.get("requestAuth") {
        None => None,
        Some(auth) => Some(parse_request_auth(auth).ok_or_else(invalid)?),
    };
    let selection_fallback = match object.get("selectionFallback") {
        None => None,
        Some(fallback) => Some(parse_selection_fallback(fallback).ok_or_else(invalid)?),
    };
    Ok(ModelExecution {
        request_auth,
        selection_fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_protocol_shape_strictly() {
        let full = json!({
            "selectionScope":"execution",
            "requestAuth":{"apiKey":"k","apiKeyId":"id","accountScope":"a".repeat(64),"headers":{"X-A":"b"}},
            "selectionFallback":{"providerId":"highspeed","rules":[{"reason":"highspeed_request_failed","providerErrorCode":"3402"}],"target":{"providerId":"account","modelId":"m","options":{"reasoningLevel":"high"}}}
        });
        let parsed = parse(&full).unwrap();
        assert!(parsed.request_auth.is_some() && parsed.selection_fallback.is_some());
        assert_eq!(
            parse(&json!({"selectionScope":"execution"})).unwrap(),
            ModelExecution::default()
        );
        for bad in [
            json!({}),
            json!({"selectionScope":"session"}),
            json!({"selectionScope":"execution","extra":1}),
            json!({"selectionScope":"execution","memoryExtraction":"skip"}),
            json!({"selectionScope":"execution","subagents":{"foregroundModel":"submission","background":"deny"}}),
            json!({"selectionScope":"execution","requestAuth":{"apiKey":""}}),
            json!({"selectionScope":"execution","requestAuth":{"headers":{"X":""}}}),
            json!({"selectionScope":"execution","requestAuth":{"accountScope":"ABC"}}),
            json!({"selectionScope":"execution","selectionFallback":{"providerId":"highspeed","rules":[]}}),
            json!({"selectionScope":"execution","selectionFallback":{"providerId":"highspeed","rules":[{"reason":"unknown"}]}}),
            json!({"selectionScope":"execution","selectionFallback":{"providerId":"highspeed","rules":[{"reason":"highspeed_request_failed"}],"target":{"providerId":"account"}}}),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }
}
