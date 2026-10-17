//! `agents-state.json` 的读写与归一（TS `readAgentStateAtPath` / `writeAgentStateFile`）。
//!
//! 落盘格式 = `JSON.stringify(state, null, 2)`（**注意：没有结尾换行**，与 `commands` 的 config.json 不同）。
//! 读入归一：两个覆盖表只保留**合法**条目（内置两个名字 / `plugin:` 前缀 + 通过 modelSelection 校验），
//! `disabledAgentIds` 只留非空字符串；其余字段（含未知字段）原样保留。

use std::path::Path;

use serde_json::{Map, Value, json};

use super::model_selection;

const BUILT_IN_NAMES: [&str; 2] = ["general-purpose", "Explore"];

pub fn empty_state() -> Value {
    json!({
        "builtInModelSelectionOverrides": {},
        "pluginAgentModelSelectionOverrides": {},
        "disabledAgentIds": [],
    })
}

/// TS `readAgentStateAtPath`：文件缺失或 JSON 语法错误 → 空状态；其它 IO 错误照常抛。
pub async fn read_state(path: &Path) -> Result<Value, String> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty_state()),
        Err(error) => return Err(format!("{error}")),
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        // TS：`SyntaxError` 沿用空覆盖语义（不覆盖原文、不报错）。
        return Ok(empty_state());
    };
    let Some(object) = parsed.as_object() else {
        return Ok(empty_state());
    };
    Ok(normalize(object))
}

fn normalize(object: &Map<String, Value>) -> Value {
    let mut next = object.clone();
    next.insert(
        "builtInModelSelectionOverrides".to_owned(),
        normalize_built_in(object.get("builtInModelSelectionOverrides")),
    );
    next.insert(
        "pluginAgentModelSelectionOverrides".to_owned(),
        normalize_plugin(object.get("pluginAgentModelSelectionOverrides")),
    );
    next.insert(
        "disabledAgentIds".to_owned(),
        normalize_disabled(object.get("disabledAgentIds")),
    );
    Value::Object(next)
}

/// 内置覆盖只认两个固定名字；条目必须通过 modelSelection 校验。
fn normalize_built_in(raw: Option<&Value>) -> Value {
    let mut out = Map::new();
    for name in BUILT_IN_NAMES {
        if let Ok(Some(selection)) =
            model_selection::normalize(raw.and_then(|value| value.get(name)))
        {
            out.insert(name.to_owned(), selection);
        }
    }
    Value::Object(out)
}

/// 插件覆盖只认 `plugin:` 前缀的 id。
fn normalize_plugin(raw: Option<&Value>) -> Value {
    let mut out = Map::new();
    let Some(object) = raw.and_then(Value::as_object) else {
        return Value::Object(out);
    };
    for (id, candidate) in object {
        if !id.starts_with("plugin:") {
            continue;
        }
        if let Ok(Some(selection)) = model_selection::normalize(Some(candidate)) {
            out.insert(id.clone(), selection);
        }
    }
    Value::Object(out)
}

fn normalize_disabled(raw: Option<&Value>) -> Value {
    let ids: Vec<Value> = raw
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(|id| Value::from(id.to_owned()))
                .collect()
        })
        .unwrap_or_default();
    Value::Array(ids)
}

/// TS `writeAgentStateFile`：`JSON.stringify(state, null, 2)`，2 空格缩进、**无结尾换行**。
pub async fn write_state(path: &Path, state: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| format!("{error}"))?;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || escode_host_core::atomic_write::write(&path, &bytes))
        .await
        .map_err(|error| format!("{error}"))?
        .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
