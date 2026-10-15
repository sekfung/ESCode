//! 配置问题清单（TS `validateConfigSchema` 对 `complete*DataSchema` 的翻译）。
//!
//! 能通过文件加载（稀疏 schema）的配置，完整性不合格的原因只有两类：必填叶子缺失（缺字段或 `null`），以及
//! 值域在“暂存编辑”中被放宽的字段（Personal 的 `baseUrl`、空白 `apiKey`）。issue 顺序 = zod 对象键声明顺序。

use serde_json::{Map, Value, json};

use super::complete::{model_complete, provider_complete};
use crate::git::paths::js_trim;

pub type Issue = Value;

fn path_text(path: &[String]) -> String {
    path.join(".")
}

fn issue(code: &str, path: &[String], message: String) -> Issue {
    json!({"code": code, "path": path, "message": message})
}

fn missing(path: &[String], field: &str) -> Issue {
    let mut full = path.to_vec();
    full.push(field.to_owned());
    let message = format!("缺少必填配置 {}", path_text(&full));
    issue("required-field-missing", &full, message)
}

fn absent(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).is_none_or(Value::is_null)
}

fn child(path: &[String], field: &str) -> Vec<String> {
    let mut full = path.to_vec();
    full.push(field.to_owned());
    full
}

/// TS `ProviderConfig.validateComplete(path)`。
pub fn provider_issues(config: &Value, path: &[String]) -> Vec<Issue> {
    let mut out = Vec::new();
    let empty = Map::new();
    let map = config.as_object().unwrap_or(&empty);
    if absent(map, "group") {
        out.push(missing(path, "group"));
    }
    match map.get("access").filter(|v| !v.is_null()) {
        None => out.push(missing(path, "access")),
        Some(Value::Object(access)) => {
            let access_path = child(path, "access");
            match access.get("type").and_then(Value::as_str) {
                Some("api-key" | "zhipu-coding-plan-api-key") => {
                    let blank = access
                        .get("apiKey")
                        .and_then(Value::as_str)
                        .is_none_or(|key| js_trim(key).is_empty());
                    if blank {
                        out.push(missing(&access_path, "apiKey"));
                    }
                }
                Some("zhipu-account") => {
                    for field in ["accountType", "mode", "entitled"] {
                        if absent(access, field) {
                            out.push(missing(&access_path, field));
                        }
                    }
                }
                _ => {}
            }
        }
        Some(_) => {}
    }
    match map.get("api").filter(|v| !v.is_null()) {
        None => out.push(missing(path, "api")),
        Some(Value::Object(api)) => {
            let api_path = child(path, "api");
            if absent(api, "type") {
                out.push(missing(&api_path, "type"));
            }
            match api.get("baseUrl").and_then(Value::as_str) {
                Some(url) if !js_trim(url).is_empty() => {
                    if url::Url::parse(url).is_err() {
                        let full = child(&api_path, "baseUrl");
                        let message = format!("配置 {} 必须是有效 URL", path_text(&full));
                        out.push(issue("invalid-url", &full, message));
                    }
                }
                _ => out.push(missing(&api_path, "baseUrl")),
            }
        }
        Some(_) => {}
    }
    if out.is_empty() && !provider_complete(config) {
        out.push(issue("invalid-config", path, "配置无效".to_owned()));
    }
    out
}

/// TS `ModelConfig.validateComplete(path)`。
pub fn model_issues(config: &Value, path: &[String]) -> Vec<Issue> {
    let mut out = Vec::new();
    let empty = Map::new();
    let map = config.as_object().unwrap_or(&empty);
    if absent(map, "enabled") {
        out.push(missing(path, "enabled"));
    }
    match map.get("properties").filter(|v| !v.is_null()) {
        None => out.push(missing(path, "properties")),
        Some(Value::Object(properties)) => {
            let props_path = child(path, "properties");
            for field in ["requiresMfjsToolSchema", "contextWindow"] {
                if absent(properties, field) {
                    out.push(missing(&props_path, field));
                }
            }
            match properties.get("inputFormat").filter(|v| !v.is_null()) {
                None => out.push(missing(&props_path, "inputFormat")),
                Some(Value::Object(format)) => {
                    let format_path = child(&props_path, "inputFormat");
                    for field in [
                        "supportsText",
                        "supportsImage",
                        "supportsVideo",
                        "supportsAudio",
                        "supportsPdf",
                    ] {
                        if absent(format, field) {
                            out.push(missing(&format_path, field));
                        }
                    }
                }
                Some(_) => {}
            }
            match properties.get("outputFormat").filter(|v| !v.is_null()) {
                None => out.push(missing(&props_path, "outputFormat")),
                Some(Value::Object(format)) => {
                    if absent(format, "supportsText") {
                        out.push(missing(&child(&props_path, "outputFormat"), "supportsText"));
                    }
                }
                Some(_) => {}
            }
            for field in [
                "supportsToolCall",
                "supportsJsonSchemaOutput",
                "supportsNativeWebSearch",
                "supportsMidConversationSystem",
            ] {
                if absent(properties, field) {
                    out.push(missing(&props_path, field));
                }
            }
        }
        Some(_) => {}
    }
    match map.get("optionSpecs").filter(|v| !v.is_null()) {
        None => out.push(missing(path, "optionSpecs")),
        Some(Value::Object(specs)) => {
            let specs_path = child(path, "optionSpecs");
            for (name, fields) in [
                ("reasoningLevel", ["values", "map"]),
                ("maxOutputTokens", ["max", "map"]),
            ] {
                match specs.get(name).filter(|v| !v.is_null()) {
                    None => out.push(missing(&specs_path, name)),
                    Some(Value::Object(spec)) => {
                        let spec_path = child(&specs_path, name);
                        for field in fields {
                            if absent(spec, field) {
                                out.push(missing(&spec_path, field));
                            }
                        }
                    }
                    Some(_) => {}
                }
            }
        }
        Some(_) => {}
    }
    if out.is_empty() && !model_complete(config) {
        out.push(issue("invalid-config", path, "配置无效".to_owned()));
    }
    out
}
