//! 写入入口的稀疏配置校验（TS `providerConfigDataSchema` / `modelConfigDataSchema` /
//! `manualModelConfigSchema` 的结构与值域部分）。
//!
//! 与 TS 的差异（登记在 docs/specs/rust-u2-providers.md §8）：非法输入的错误文案是中文一句话
//! （`路径: 原因`），不是 zod 的 JSON issue 列表；`optionSpecs.*.map` 只要求非空字符串，
//! 不做 `compileModelOptionMap` 的编译检查。

use serde_json::{Map, Value};

use crate::git::paths::js_trim;

pub type Check = Result<(), String>;

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path}: 必须是对象"))
}

fn strict(map: &Map<String, Value>, allowed: &[&str], path: &str) -> Check {
    match map.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(extra) => Err(format!("{path}: 含未知字段 {extra}")),
        None => Ok(()),
    }
}

/// `x.nullable().optional()`：缺省与 null 都放行，其余交给 `check`。
fn optional(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    check: fn(&Value, &str) -> Check,
) -> Check {
    match map.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(value) => check(value, &format!("{path}.{key}")),
    }
}

fn boolean(value: &Value, path: &str) -> Check {
    if value.is_boolean() {
        Ok(())
    } else {
        Err(format!("{path}: 必须是 boolean"))
    }
}

fn nonempty_string(value: &Value, path: &str) -> Check {
    match value.as_str() {
        Some(text) if !text.is_empty() => Ok(()),
        _ => Err(format!("{path}: 必须是非空字符串")),
    }
}

fn plain_string(value: &Value, path: &str) -> Check {
    if value.is_string() {
        Ok(())
    } else {
        Err(format!("{path}: 必须是字符串"))
    }
}

fn url_string(value: &Value, path: &str) -> Check {
    match value.as_str() {
        Some(text) if url::Url::parse(text).is_ok() => Ok(()),
        _ => Err(format!("{path}: 必须是合法 URL")),
    }
}

fn positive_int(value: &Value, path: &str) -> Check {
    let ok = value.as_u64().is_some_and(|n| n > 0)
        || value
            .as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0 && f > 0.0);
    if ok {
        Ok(())
    } else {
        Err(format!("{path}: 必须是正整数"))
    }
}

fn one_of(value: &Value, path: &str, allowed: &[&str]) -> Check {
    match value.as_str() {
        Some(text) if allowed.contains(&text) => Ok(()),
        _ => Err(format!("{path}: 必须是 {} 之一", allowed.join("/"))),
    }
}

fn model_ids(value: &Value, path: &str) -> Check {
    let Some(items) = value.as_array() else {
        return Err(format!("{path}: 必须是数组"));
    };
    items
        .iter()
        .try_for_each(|item| nonempty_string(item, path))
}

fn logo(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(map, &["type", "key"], path)?;
    one_of(
        map.get("type").unwrap_or(&Value::Null),
        &format!("{path}.type"),
        &["builtin"],
    )?;
    nonempty_string(
        map.get("key").unwrap_or(&Value::Null),
        &format!("{path}.key"),
    )
}

fn access(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    let type_path = format!("{path}.type");
    match map.get("type").and_then(Value::as_str) {
        Some("api-key" | "zhipu-coding-plan-api-key") => {
            strict(map, &["type", "apiKey", "apiKeyManagementUrl"], path)?;
            optional(map, "apiKey", path, plain_string)?;
            optional(map, "apiKeyManagementUrl", path, url_string)
        }
        Some("zhipu-account") => {
            strict(map, &["type", "accountType", "mode", "entitled"], path)?;
            if let Some(value) = map.get("accountType").filter(|v| !v.is_null()) {
                one_of(value, &format!("{path}.accountType"), &["zai", "bigmodel"])?;
            }
            if let Some(value) = map.get("mode").filter(|v| !v.is_null()) {
                one_of(
                    value,
                    &format!("{path}.mode"),
                    &[
                        "start-plan",
                        "individual-coding-plan",
                        "team-coding-plan",
                        "off-peak",
                        "highspeed",
                    ],
                )?;
            }
            optional(map, "entitled", path, boolean)
        }
        _ => Err(format!(
            "{type_path}: 必须是 api-key/zhipu-coding-plan-api-key/zhipu-account"
        )),
    }
}

fn api(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(map, &["type", "baseUrl", "headers"], path)?;
    if let Some(value) = map.get("type").filter(|v| !v.is_null()) {
        one_of(
            value,
            &format!("{path}.type"),
            &[
                "anthropic-messages",
                "openai-chat-completions",
                "openai-responses",
            ],
        )?;
    }
    optional(map, "baseUrl", path, url_string)?;
    match map.get("headers") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(headers)) => headers
            .values()
            .try_for_each(|header| plain_string(header, &format!("{path}.headers"))),
        Some(_) => Err(format!("{path}.headers: 必须是字符串字典")),
    }
}

/// `providerConfigDataSchema`（strict；全部字段可缺省/null）。
pub fn provider_config(value: &Value) -> Check {
    let path = "config";
    let map = object(value, path)?;
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
        path,
    )?;
    if let Some(group) = map.get("group").filter(|v| !v.is_null()) {
        one_of(
            group,
            "config.group",
            &["standard-personal", "zai-family", "bigmodel-family"],
        )?;
    }
    if let Some(visibility) = map.get("visibility").filter(|v| !v.is_null()) {
        one_of(visibility, "config.visibility", &["visible", "hidden"])?;
    }
    optional(map, "logo", path, logo)?;
    optional(map, "access", path, access)?;
    optional(map, "api", path, api)?;
    for key in ["builtinModelIds", "personalModelIds", "modelOrder"] {
        optional(map, key, path, model_ids)?;
    }
    Ok(())
}

fn input_format(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(
        map,
        &[
            "supportsText",
            "supportsImage",
            "supportsVideo",
            "supportsAudio",
            "supportsPdf",
        ],
        path,
    )?;
    for key in [
        "supportsText",
        "supportsImage",
        "supportsVideo",
        "supportsAudio",
        "supportsPdf",
    ] {
        optional(map, key, path, boolean)?;
    }
    Ok(())
}

fn output_format(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(map, &["supportsText"], path)?;
    optional(map, "supportsText", path, boolean)
}

fn properties(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(
        map,
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
        path,
    )?;
    for key in [
        "requiresMfjsToolSchema",
        "supportsToolCall",
        "supportsJsonSchemaOutput",
        "supportsNativeWebSearch",
        "supportsMidConversationSystem",
    ] {
        optional(map, key, path, boolean)?;
    }
    optional(map, "contextWindow", path, positive_int)?;
    optional(map, "inputFormat", path, input_format)?;
    optional(map, "outputFormat", path, output_format)
}

fn reasoning_level(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(map, &["values", "map"], path)?;
    if let Some(values) = map.get("values").filter(|v| !v.is_null()) {
        let items = values
            .as_array()
            .filter(|items| !items.is_empty())
            .ok_or_else(|| format!("{path}.values: 必须是非空数组"))?;
        let mut seen = std::collections::HashSet::new();
        for item in items {
            let text = item
                .as_str()
                .filter(|text| !js_trim(text).is_empty())
                .ok_or_else(|| format!("{path}.values: 元素必须是非空字符串"))?;
            if !seen.insert(text) {
                return Err(format!("{path}.values: 不能重复"));
            }
        }
    }
    optional(map, "map", path, nonempty_string)
}

fn max_output_tokens(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(map, &["max", "map"], path)?;
    optional(map, "max", path, positive_int)?;
    optional(map, "map", path, nonempty_string)
}

fn option_specs(value: &Value, path: &str) -> Check {
    let map = object(value, path)?;
    strict(map, &["reasoningLevel", "maxOutputTokens"], path)?;
    optional(map, "reasoningLevel", path, reasoning_level)?;
    optional(map, "maxOutputTokens", path, max_output_tokens)
}

/// `modelConfigDataSchema`（strict；全部字段可缺省/null）。
pub fn model_config(value: &Value) -> Check {
    let path = "modelConfig";
    let map = object(value, path)?;
    strict(map, &["enabled", "properties", "optionSpecs"], path)?;
    optional(map, "enabled", path, boolean)?;
    optional(map, "properties", path, properties)?;
    optional(map, "optionSpecs", path, option_specs)
}

fn required<'a>(map: &'a Map<String, Value>, key: &str, path: &str) -> Result<&'a Value, String> {
    map.get(key)
        .filter(|value| !value.is_null())
        .ok_or_else(|| format!("{path}.{key}: 手动配置必填"))
}

/// `manualModelConfigSchema`：手动模式的可编辑叶子必须齐全（`enabled` 仍可缺省）。
pub fn manual_model_config(value: &Value) -> Check {
    let path = "manualModelConfig";
    let map = object(value, path)?;
    strict(map, &["enabled", "properties", "optionSpecs"], path)?;
    optional(map, "enabled", path, boolean)?;

    let props_path = format!("{path}.properties");
    let props = object(required(map, "properties", path)?, &props_path)?;
    strict(
        props,
        &[
            "contextWindow",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
            "inputFormat",
        ],
        &props_path,
    )?;
    positive_int(
        required(props, "contextWindow", &props_path)?,
        &format!("{props_path}.contextWindow"),
    )?;
    for key in [
        "supportsJsonSchemaOutput",
        "supportsNativeWebSearch",
        "supportsMidConversationSystem",
    ] {
        boolean(
            required(props, key, &props_path)?,
            &format!("{props_path}.{key}"),
        )?;
    }
    let format_path = format!("{props_path}.inputFormat");
    let format = object(required(props, "inputFormat", &props_path)?, &format_path)?;
    strict(
        format,
        &["supportsImage", "supportsVideo", "supportsPdf"],
        &format_path,
    )?;
    for key in ["supportsImage", "supportsVideo", "supportsPdf"] {
        boolean(
            required(format, key, &format_path)?,
            &format!("{format_path}.{key}"),
        )?;
    }

    let specs_path = format!("{path}.optionSpecs");
    let specs = object(required(map, "optionSpecs", path)?, &specs_path)?;
    strict(specs, &["reasoningLevel", "maxOutputTokens"], &specs_path)?;
    let level_path = format!("{specs_path}.reasoningLevel");
    let level = required(specs, "reasoningLevel", &specs_path)?;
    reasoning_level(level, &level_path)?;
    let level_map = object(level, &level_path)?;
    required(level_map, "values", &level_path)?;
    nonempty_string(
        required(level_map, "map", &level_path)?,
        &format!("{level_path}.map"),
    )?;
    let limit_path = format!("{specs_path}.maxOutputTokens");
    let limit = object(
        required(specs, "maxOutputTokens", &specs_path)?,
        &limit_path,
    )?;
    strict(limit, &["max"], &limit_path)?;
    positive_int(
        required(limit, "max", &limit_path)?,
        &format!("{limit_path}.max"),
    )
}

#[cfg(test)]
#[path = "sparse_tests.rs"]
mod tests;
