//! 完整性校验（TS `validateComplete`：zod 的 `complete*DataSchema`）。P1 只需要“是否合格”——
//! 不合格的 Provider / Model 不进入 Registry；逐条 `issues` 文案随 P2（设置视图）补齐。

use serde_json::{Map, Value};

use crate::git::paths::js_trim;

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// strict 对象：不得出现 `allowed` 之外的键。
fn strict(map: &Map<String, Value>, allowed: &[&str]) -> bool {
    map.keys().all(|key| allowed.contains(&key.as_str()))
}

fn nonblank_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !js_trim(text).is_empty())
}

/// zod `z.string().url()`：能被 WHATWG URL 解析。
fn is_url(text: &str) -> bool {
    url::Url::parse(text).is_ok()
}

/// 可选 + 可空的字符串数组（元素非空）。
fn model_ids(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) => items
            .iter()
            .all(|item| item.as_str().is_some_and(|s| !s.is_empty())),
        _ => false,
    }
}

fn is_int_positive(value: Option<&Value>) -> bool {
    value.is_some_and(|v| {
        v.as_i64().is_some_and(|n| n > 0)
            || v.as_u64().is_some_and(|n| n > 0)
            || v.as_f64()
                .is_some_and(|f| f.fract() == 0.0 && f > 0.0 && f.is_finite())
    })
}

fn access_ok(access: &Value) -> bool {
    let Some(map) = object(access) else {
        return false;
    };
    match map.get("type").and_then(Value::as_str) {
        Some("api-key" | "zhipu-coding-plan-api-key") => {
            strict(map, &["type", "apiKey", "apiKeyManagementUrl"])
                && nonblank_string(map.get("apiKey"))
                && match map.get("apiKeyManagementUrl") {
                    None | Some(Value::Null) => true,
                    Some(Value::String(text)) => is_url(text),
                    _ => false,
                }
        }
        Some("zhipu-account") => {
            strict(map, &["type", "accountType", "mode", "entitled"])
                && matches!(
                    map.get("accountType").and_then(Value::as_str),
                    Some("zai" | "bigmodel")
                )
                && matches!(
                    map.get("mode").and_then(Value::as_str),
                    Some(
                        "start-plan"
                            | "individual-coding-plan"
                            | "team-coding-plan"
                            | "off-peak"
                            | "highspeed"
                    )
                )
                && map.get("entitled").is_some_and(Value::is_boolean)
        }
        _ => false,
    }
}

fn api_ok(api: &Value) -> bool {
    let Some(map) = object(api) else { return false };
    strict(map, &["type", "baseUrl", "headers"])
        && matches!(
            map.get("type").and_then(Value::as_str),
            Some("anthropic-messages" | "openai-chat-completions" | "openai-responses")
        )
        && nonblank_string(map.get("baseUrl"))
        && map
            .get("baseUrl")
            .and_then(Value::as_str)
            .is_some_and(is_url)
        && match map.get("headers") {
            None | Some(Value::Null) => true,
            Some(Value::Object(headers)) => headers.values().all(Value::is_string),
            _ => false,
        }
}

/// TS `completeProviderConfigDataSchema`。
pub fn provider_complete(config: &Value) -> bool {
    let Some(map) = object(config) else {
        return false;
    };
    strict(
        map,
        &[
            "group",
            "logo",
            "access",
            "api",
            "builtinModelIds",
            "personalModelIds",
            "modelOrder",
            "visibility",
        ],
    ) && matches!(
        map.get("group").and_then(Value::as_str),
        Some("standard-personal" | "zai-family" | "bigmodel-family")
    ) && match map.get("logo") {
        None | Some(Value::Null) => true,
        Some(Value::Object(logo)) => {
            strict(logo, &["type", "key"])
                && logo.get("type").and_then(Value::as_str) == Some("builtin")
                && logo
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(|k| !k.is_empty())
        }
        _ => false,
    } && map.get("access").is_some_and(access_ok)
        && map.get("api").is_some_and(api_ok)
        && model_ids(map.get("builtinModelIds"))
        && model_ids(map.get("personalModelIds"))
        && model_ids(map.get("modelOrder"))
        && matches!(
            map.get("visibility"),
            None | Some(Value::Null) | Some(Value::String(_))
        )
        && map
            .get("visibility")
            .and_then(Value::as_str)
            .is_none_or(|v| v == "visible" || v == "hidden")
}

fn bools(map: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter()
        .all(|key| map.get(*key).is_some_and(Value::is_boolean))
}

/// option map 能被解析（TS `compileModelOptionMap`）：以代表性输入试算，解析/语义错误即不合格。
fn option_map_ok(map: Option<&Value>, variable: &str, samples: &[Value]) -> bool {
    let Some(source) = map.and_then(Value::as_str) else {
        return false;
    };
    !source.is_empty()
        && samples
            .iter()
            .all(|sample| escode_cli_domain::option_map::evaluate(source, variable, sample).is_ok())
}

fn reasoning_ok(spec: &Value) -> bool {
    let Some(map) = object(spec) else {
        return false;
    };
    if !strict(map, &["values", "map"]) {
        return false;
    }
    let Some(values) = map.get("values").and_then(Value::as_array) else {
        return false;
    };
    if values.is_empty()
        || !values
            .iter()
            .all(|v| v.as_str().is_some_and(|s| !js_trim(s).is_empty()))
    {
        return false;
    }
    let mut seen = std::collections::HashSet::new();
    values
        .iter()
        .filter_map(Value::as_str)
        .all(|s| seen.insert(s))
        && option_map_ok(map.get("map"), "reasoningLevel", values)
}

fn max_output_ok(spec: &Value) -> bool {
    let Some(map) = object(spec) else {
        return false;
    };
    strict(map, &["max", "map"])
        && is_int_positive(map.get("max"))
        && option_map_ok(map.get("map"), "maxOutputTokens", &[map["max"].clone()])
}

/// TS `completeModelConfigDataSchema`。
pub fn model_complete(config: &Value) -> bool {
    let Some(map) = object(config) else {
        return false;
    };
    if !strict(map, &["enabled", "properties", "optionSpecs"])
        || !map.get("enabled").is_some_and(Value::is_boolean)
    {
        return false;
    }
    let Some(properties) = map.get("properties").and_then(object) else {
        return false;
    };
    let Some(specs) = map.get("optionSpecs").and_then(object) else {
        return false;
    };
    strict(
        properties,
        &[
            "requiresMfjsToolSchema",
            "contextWindow",
            "inputFormat",
            "outputFormat",
            "supportsToolCall",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
        ],
    ) && bools(
        properties,
        &[
            "requiresMfjsToolSchema",
            "supportsToolCall",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
        ],
    ) && is_int_positive(properties.get("contextWindow"))
        && properties
            .get("inputFormat")
            .and_then(object)
            .is_some_and(|f| {
                strict(
                    f,
                    &[
                        "supportsText",
                        "supportsImage",
                        "supportsVideo",
                        "supportsAudio",
                        "supportsPdf",
                    ],
                ) && bools(
                    f,
                    &[
                        "supportsText",
                        "supportsImage",
                        "supportsVideo",
                        "supportsAudio",
                        "supportsPdf",
                    ],
                )
            })
        && properties
            .get("outputFormat")
            .and_then(object)
            .is_some_and(|f| strict(f, &["supportsText"]) && bools(f, &["supportsText"]))
        && strict(specs, &["reasoningLevel", "maxOutputTokens"])
        && specs.get("reasoningLevel").is_some_and(reasoning_ok)
        && specs.get("maxOutputTokens").is_some_and(max_output_ok)
}

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
