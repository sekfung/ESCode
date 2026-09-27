//! 单次执行约束（协议 `modelExecutionSchema`，strict），docs/specs/rust-offpeak.md 第三期。
//! 凭据只属于本次执行：调用方只在内存保存，不写入输入 payload、数据库、事件或日志。

use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelExecution {
    /// `memoryExtraction: "skip"`：本轮结束不触发项目记忆提取。
    pub skip_memory: bool,
    /// `requestAuth {apiKey?, headers?}`：off-peak 账号模型的本轮鉴权材料。
    pub request_auth: Option<Value>,
    /// `subagents {foregroundModel: "submission", background: "deny"}`。
    pub subagents: bool,
}

fn strict(value: &Value, keys: &[&str]) -> Option<Map<String, Value>> {
    let object = value.as_object()?;
    object.keys().all(|k| keys.contains(&k.as_str())).then(|| object.clone())
}

fn non_empty(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.is_empty())
}

pub fn parse(value: &Value) -> Result<ModelExecution, String> {
    let invalid = || "Invalid modelExecution".to_owned();
    let object = strict(value, &["memoryExtraction", "selectionScope", "requestAuth", "subagents"]).ok_or_else(invalid)?;
    if object.get("selectionScope").and_then(Value::as_str) != Some("execution") {
        return Err(invalid());
    }
    let skip_memory = match object.get("memoryExtraction") {
        None => false,
        Some(v) if v == "skip" => true,
        Some(_) => return Err(invalid()),
    };
    let request_auth = match object.get("requestAuth") {
        None => None,
        Some(auth) => {
            let auth = strict(auth, &["apiKey", "headers"]).ok_or_else(invalid)?;
            if auth.get("apiKey").is_some_and(|k| !non_empty(k)) {
                return Err(invalid());
            }
            if let Some(headers) = auth.get("headers") {
                let headers = headers.as_object().ok_or_else(invalid)?;
                if headers.iter().any(|(name, value)| name.is_empty() || !non_empty(value)) {
                    return Err(invalid());
                }
            }
            Some(Value::Object(auth))
        }
    };
    let subagents = match object.get("subagents") {
        None => false,
        Some(policy) => {
            let policy = strict(policy, &["foregroundModel", "background"]).ok_or_else(invalid)?;
            if policy.get("foregroundModel").and_then(Value::as_str) != Some("submission")
                || policy.get("background").and_then(Value::as_str) != Some("deny")
            {
                return Err(invalid());
            }
            true
        }
    };
    Ok(ModelExecution { skip_memory, request_auth, subagents })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_protocol_shape_strictly() {
        let full = json!({"memoryExtraction":"skip","selectionScope":"execution","requestAuth":{"apiKey":"k","headers":{"X-A":"b"}},"subagents":{"foregroundModel":"submission","background":"deny"}});
        let parsed = parse(&full).unwrap();
        assert!(parsed.skip_memory && parsed.subagents);
        assert_eq!(parsed.request_auth, Some(json!({"apiKey":"k","headers":{"X-A":"b"}})));
        assert_eq!(parse(&json!({"selectionScope":"execution"})).unwrap(), ModelExecution::default());
        for bad in [
            json!({}),
            json!({"selectionScope":"session"}),
            json!({"selectionScope":"execution","extra":1}),
            json!({"selectionScope":"execution","memoryExtraction":"run"}),
            json!({"selectionScope":"execution","requestAuth":{"apiKey":""}}),
            json!({"selectionScope":"execution","requestAuth":{"headers":{"X":""}}}),
            json!({"selectionScope":"execution","subagents":{"foregroundModel":"submission"}}),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }
}
