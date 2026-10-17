//! `modelSelection` 的校验与归一（TS `modelSelectionSchema` + `normalizeSubagentModelSelection`）。
//!
//! TS 用 zod：`providerId`/`modelId` 必须 `trim().min(1)`；`options` 是 **strict** 的
//! `{reasoningLevel?: string.trim().min(1)}`；`reasoningLevel` 为空时不带 `options` 键。

use serde_json::{Map, Value};

fn trimmed_non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// `None` / `null` → `None`（表示「清除覆盖」）；校验失败 → `Err`。
pub fn normalize(selection: Option<&Value>) -> Result<Option<Value>, String> {
    let Some(selection) = selection.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(object) = selection.as_object() else {
        return Err("modelSelection: Expected object".to_owned());
    };
    if let Some(extra) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "providerId" | "modelId" | "options"))
    {
        return Err(format!("Unrecognized key: \"{extra}\""));
    }
    let provider_id = trimmed_non_empty(object.get("providerId")).ok_or("providerId: Required")?;
    let model_id = trimmed_non_empty(object.get("modelId")).ok_or("modelId: Required")?;

    let mut normalized = Map::new();
    normalized.insert("providerId".to_owned(), Value::from(provider_id));
    normalized.insert("modelId".to_owned(), Value::from(model_id));
    if let Some(options) = object.get("options").filter(|value| !value.is_null()) {
        let Some(options) = options.as_object() else {
            return Err("options: Expected object".to_owned());
        };
        if let Some(extra) = options.keys().find(|key| key.as_str() != "reasoningLevel") {
            return Err(format!("options: Unrecognized key: \"{extra}\""));
        }
        if let Some(level) = trimmed_non_empty(options.get("reasoningLevel")) {
            let mut nested = Map::new();
            nested.insert("reasoningLevel".to_owned(), Value::from(level));
            normalized.insert("options".to_owned(), Value::Object(nested));
        }
    }
    Ok(Some(Value::Object(normalized)))
}

#[cfg(test)]
#[path = "model_selection_tests.rs"]
mod tests;
