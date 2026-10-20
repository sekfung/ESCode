//! 会话设置 → `configOptions` 投影（`getTaskConfigOptions` / 快照的 `configOptions`）。
//!
//! 上游真相源：`packages/shared/src/escode-agent-model-state.ts` 的
//! `escodeSessionSettingsToESCodeConfigOptions` / `getESCodeAgentModeSelectOptions` /
//! `normalizeAvailableESCodeMode` 与 `packages/shared/src/model-selection.ts` 的
//! `formatModelPickerValue`。
//!
//! 模式目录是 Desktop/远程 workspace Composer、Automation 表单共用的**唯一事实源**：
//! 菜单隐藏 `edit` 不等于删除历史值，`normalize_available_mode` 必须保留 `edit`。

use serde_json::{Map, Value};

pub const MODEL_CONFIG_ID: &str = "model";
pub const MODE_CONFIG_ID: &str = "mode";
pub const THOUGHT_LEVEL_CONFIG_ID: &str = "thought_level";

/// TS `ESCODE_AGENT_MODE_OPTIONS`（id/name/description 逐字对齐，含 U+2019 撇号）。
const AGENT_MODE_OPTIONS: [(&str, &str, &str); 4] = [
    (
        "build",
        "Ask before changes",
        "Ask before each file changes.",
    ),
    ("guarded", "Autonomous mode", "Ask when there\u{2019}s risk"),
    (
        "plan",
        "Plan mode",
        "Inspect the code and present a plan before editing.",
    ),
    (
        "yolo",
        "Full access",
        "Edit and run commands with fewer confirmations.",
    ),
];

/// TS `formatModelPickerValue`：`provider/model` + 可选 `$reasoningLevel`（展示态，不可逆）。
pub fn format_model_picker_value(selection: Option<&Value>) -> String {
    let Some(selection) = selection.filter(|value| !value.is_null()) else {
        return String::new();
    };
    let provider_id = text_field(selection, "providerId");
    let model_id = text_field(selection, "modelId");
    let base = format!(
        "{}/{}",
        provider_id.unwrap_or_default(),
        model_id.unwrap_or_default()
    );
    // TS 用真值判定：空档位不加 `$` 后缀。
    match field(selection, "options")
        .and_then(|options| text_field(options, "reasoningLevel"))
        .filter(|level| !level.is_empty())
    {
        Some(level) => format!("{base}${level}"),
        None => base,
    }
}

/// TS `normalizeAvailableESCodeMode`：白名单外一律 `build`（`edit` 是保留的历史值）。
pub fn normalize_available_mode(mode: &str) -> String {
    match mode {
        "build" | "guarded" | "plan" | "yolo" | "edit" => mode.to_owned(),
        _ => "build".to_owned(),
    }
}

/// TS `escodeSessionSettingsToESCodeConfigOptions`。
pub fn settings_to_config_options(settings: &Value) -> Vec<Value> {
    let model = field(settings, "model").unwrap_or(&Value::Null);
    let thought_level = field(settings, "thoughtLevel").unwrap_or(&Value::Null);
    let mode = field(settings, "mode").unwrap_or(&Value::Null);

    let available = field(model, "available")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let model_options = available.iter().map(model_option).collect::<Vec<Value>>();

    let mut config_options = vec![
        build_config_option(
            MODEL_CONFIG_ID,
            MODEL_CONFIG_ID,
            "Model",
            format_model_picker_value(field(model, "current")),
            model_options,
        ),
        build_config_option(
            MODE_CONFIG_ID,
            MODE_CONFIG_ID,
            "Mode",
            normalize_available_mode(
                &string_field(mode, "current").unwrap_or_else(|| "build".to_owned()),
            ),
            mode_select_options(),
        ),
    ];

    if field(thought_level, "enabled").and_then(Value::as_bool) == Some(true) {
        let available = field(thought_level, "available")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let values = available
            .iter()
            .filter_map(|level| text_field(level, "value"))
            .collect::<Vec<_>>();
        // TS `settings.thoughtLevel.defaultLevel && values.has(...)`：真值判定（空串不算命中）。
        let default_level = text_field(thought_level, "defaultLevel")
            .filter(|level| !level.is_empty())
            .filter(|level| values.iter().any(|value| value == level));
        let current = text_field(thought_level, "current")
            .or(default_level)
            .or_else(|| values.first().cloned())
            .unwrap_or_default();
        let options = available
            .iter()
            .map(|level| {
                build_object([
                    ("value", text_field(level, "value").map(Value::from)),
                    ("name", text_field(level, "label").map(Value::from)),
                    (
                        "description",
                        text_field(level, "description").map(Value::from),
                    ),
                ])
            })
            .collect();
        config_options.push(build_config_option(
            THOUGHT_LEVEL_CONFIG_ID,
            THOUGHT_LEVEL_CONFIG_ID,
            "Thought Level",
            current,
            options,
        ));
    }

    config_options
}

/// TS `getESCodeAgentModeSelectOptions`。
fn mode_select_options() -> Vec<Value> {
    AGENT_MODE_OPTIONS
        .iter()
        .map(|(id, name, description)| {
            serde_json::json!({"value": id, "name": name, "description": description})
        })
        .collect()
}

/// 一条模型候选：`reasoning` 缺省时不写 `modelThoughtLevels`/`modelDefaultThoughtLevel`。
fn model_option(model: &Value) -> Value {
    let reference = field(model, "ref").unwrap_or(&Value::Null);
    let reasoning = field(model, "reasoning");
    let levels = reasoning
        .and_then(|reasoning| field(reasoning, "levels"))
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(|level| text_field(level, "value"))
                .map(Value::from)
                .collect::<Vec<Value>>()
        });
    // TS `model.reasoning?.defaultLevel && modelThoughtLevels?.includes(...)`：真值判定。
    let default_level = reasoning
        .and_then(|reasoning| text_field(reasoning, "defaultLevel"))
        .filter(|level| !level.is_empty())
        .filter(|level| {
            levels
                .as_ref()
                .is_some_and(|levels| levels.iter().any(|value| value.as_str() == Some(level)))
        });
    let provider_id = text_field(reference, "providerId");
    build_object([
        (
            "value",
            Some(Value::from(format_model_picker_value(Some(reference)))),
        ),
        ("name", text_field(model, "label").map(Value::from)),
        (
            "description",
            text_field(model, "description").map(Value::from),
        ),
        ("modelProviderId", provider_id.clone().map(Value::from)),
        (
            "modelProviderName",
            Some(Value::from(
                text_field(model, "providerLabel")
                    .or(provider_id)
                    .unwrap_or_default(),
            )),
        ),
        ("modelThoughtLevels", levels.map(Value::Array)),
        ("modelDefaultThoughtLevel", default_level.map(Value::from)),
    ])
}

fn build_config_option(
    id: &str,
    category: &str,
    name: &str,
    current_value: String,
    options: Vec<Value>,
) -> Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "category": category,
        "type": "select",
        "currentValue": current_value,
        "options": options,
    })
}

fn build_object<const N: usize>(entries: [(&'static str, Option<Value>); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        if let Some(value) = value {
            map.insert(key.to_owned(), value);
        }
    }
    Value::Object(map)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    field(value, key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.is_empty())
}

/// TS `??` 口径的字符串取值：空串是**有值**（`"" ?? x` 仍是 `""`），只有 null/缺失才回退。
fn text_field(value: &Value, key: &str) -> Option<String> {
    field(value, key)
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_picker_value_joins_provider_model_and_optional_reasoning() {
        assert_eq!(
            format_model_picker_value(Some(&json!({"providerId": "glm", "modelId": "glm-4"}))),
            "glm/glm-4"
        );
        assert_eq!(
            format_model_picker_value(Some(&json!({
                "providerId": "custom", "modelId": "kimi-k2:free",
                "options": {"reasoningLevel": "high"}
            }))),
            // `:` 属于 modelId（OpenRouter 的 :free），只有 `$` 才是档位分隔符。
            "custom/kimi-k2:free$high"
        );
        assert_eq!(format_model_picker_value(None), "");
        assert_eq!(format_model_picker_value(Some(&Value::Null)), "");
    }

    #[test]
    fn mode_normalization_keeps_edit_and_falls_back_to_build() {
        assert_eq!(normalize_available_mode("edit"), "edit");
        assert_eq!(normalize_available_mode("yolo"), "yolo");
        assert_eq!(normalize_available_mode("auto"), "build");
        assert_eq!(normalize_available_mode(""), "build");
    }

    #[test]
    fn config_options_project_model_mode_and_thought_level() {
        let settings = json!({
            "model": {
                "current": {"providerId": "glm", "modelId": "glm-4", "options": {"reasoningLevel": "high"}},
                "available": [
                    {"ref": {"providerId": "glm", "modelId": "glm-4"}, "label": "GLM-4",
                     "description": "主力模型", "providerLabel": "智谱",
                     "reasoning": {"levels": [{"value": "none"}, {"value": "high"}], "defaultLevel": "high"}},
                    {"ref": {"providerId": "custom", "modelId": "kimi-k2:free"}, "label": "Kimi"},
                ],
            },
            "thoughtLevel": {
                "enabled": true, "current": "high", "defaultLevel": "medium",
                "available": [{"value": "none", "label": "None"}, {"value": "high", "label": "High", "description": "深想"}],
            },
            "mode": {"current": "plan"},
            "permission": {"mode": "plan"},
        });
        let options = settings_to_config_options(&settings);
        assert_eq!(options.len(), 3);
        assert_eq!(options[0]["id"], json!("model"));
        assert_eq!(options[0]["category"], json!("model"));
        assert_eq!(options[0]["currentValue"], json!("glm/glm-4$high"));
        assert_eq!(options[0]["options"][0]["value"], json!("glm/glm-4"));
        assert_eq!(options[0]["options"][0]["name"], json!("GLM-4"));
        assert_eq!(options[0]["options"][0]["modelProviderId"], json!("glm"));
        assert_eq!(options[0]["options"][0]["modelProviderName"], json!("智谱"));
        assert_eq!(
            options[0]["options"][0]["modelThoughtLevels"],
            json!(["none", "high"])
        );
        assert_eq!(
            options[0]["options"][0]["modelDefaultThoughtLevel"],
            json!("high")
        );
        // 无 reasoning 的模型：两个条件键都不写，providerName 回退 providerId。
        assert_eq!(
            options[0]["options"][1]["modelProviderName"],
            json!("custom")
        );
        assert!(options[0]["options"][1].get("modelThoughtLevels").is_none());
        assert!(
            options[0]["options"][1]
                .get("modelDefaultThoughtLevel")
                .is_none()
        );
        assert!(options[0]["options"][1].get("description").is_none());
        assert_eq!(options[1]["id"], json!("mode"));
        assert_eq!(options[1]["currentValue"], json!("plan"));
        assert_eq!(options[1]["options"].as_array().expect("modes").len(), 4);
        assert_eq!(
            options[1]["options"][1]["description"],
            json!("Ask when there\u{2019}s risk")
        );
        assert_eq!(options[2]["id"], json!("thought_level"));
        // defaultLevel 不在 available 里 → 回退 available[0]。
        assert_eq!(options[2]["currentValue"], json!("high"));
        assert_eq!(options[2]["options"][1]["name"], json!("High"));
    }

    #[test]
    fn thought_level_option_is_omitted_when_disabled_or_empty() {
        let disabled = settings_to_config_options(&json!({
            "model": {"current": {"providerId": "glm", "modelId": "glm-4"}, "available": []},
            "thoughtLevel": {"enabled": false, "available": [{"value": "high", "label": "High"}]},
            "mode": {"current": "build"},
        }));
        assert_eq!(disabled.len(), 2);
        assert_eq!(disabled[0]["currentValue"], json!("glm/glm-4"));
        assert_eq!(disabled[0]["options"], json!([]));
        assert_eq!(disabled[1]["currentValue"], json!("build"));
        // enabled 但 current/defaultLevel/available 全空 → currentValue 为空串。
        let empty = settings_to_config_options(&json!({
            "model": {"available": []},
            "thoughtLevel": {"enabled": true, "available": []},
            "mode": {},
        }));
        assert_eq!(empty.len(), 3);
        assert_eq!(empty[2]["currentValue"], json!(""));
        // 无 `model.current` 时 `formatModelPickerValue(undefined)` 是空串（不是 "/"）。
        assert_eq!(empty[0]["currentValue"], json!(""));
    }
}
