//! Settings 侧的 frontmatter 解析口径（TS `subagentMarkdown.ts` 的 `parseRuntimeCompatibleFrontmatter`）。
//!
//! TS **同时**跑真正的 `yaml` 包与一份宽松逐行解析，合并规则是
//! `{...yamlValues, ...looseValues, mcpServers: yaml 数组优先}`——即宽松结果覆盖 YAML，
//! 唯独 `mcpServers` 在 YAML 给出数组时以 YAML 为准。
//!
//! Rust 用 `saphyr`（YAML 1.2 core schema，与 TS `yaml` 同族）复刻这条合并。
//! **登记差异**：`yaml` 包对重复键报错，saphyr 的映射是后者覆盖；仅在**重复键**的坏文件上可见。

use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};
use serde_json::{Map, Number, Value};

use super::frontmatter_value::parse_scalar_value;

/// saphyr 标量 → JSON。整数走 i64（JS 序列化回 `42` 而不是 `42.0`）。
fn scalar_to_json(scalar: &ScalarOwned) -> Value {
    match scalar {
        ScalarOwned::Null => Value::Null,
        ScalarOwned::Boolean(flag) => Value::Bool(*flag),
        ScalarOwned::Integer(number) => Value::from(*number),
        ScalarOwned::FloatingPoint(number) => {
            Number::from_f64(number.0).map_or(Value::Null, Value::Number)
        }
        ScalarOwned::String(text) => Value::from(text.clone()),
    }
}

fn node_to_json(node: &YamlOwned) -> Value {
    match node {
        YamlOwned::Value(scalar) => scalar_to_json(scalar),
        YamlOwned::Sequence(items) => Value::Array(items.iter().map(node_to_json).collect()),
        YamlOwned::Mapping(map) => {
            let mut object = Map::new();
            for (key, value) in map {
                object.insert(mapping_key(key), node_to_json(value));
            }
            Value::Object(object)
        }
        YamlOwned::Tagged(_, inner) => node_to_json(inner),
        // 保留原始写法时按字符串处理（与 TS 得到字符串的常见情形一致）。
        YamlOwned::Representation(text, ..) => Value::from(text.clone()),
        // `YamlOwned` 还有别名占位等变体：解析不出值时按 null（TS 的 `yaml` 会解析出真实值）。
        _ => Value::Null,
    }
}

fn mapping_key(node: &YamlOwned) -> String {
    match node {
        YamlOwned::Value(ScalarOwned::String(text)) => text.clone(),
        YamlOwned::Value(ScalarOwned::Null) => String::new(),
        YamlOwned::Value(ScalarOwned::Boolean(flag)) => flag.to_string(),
        YamlOwned::Value(ScalarOwned::Integer(number)) => number.to_string(),
        YamlOwned::Value(ScalarOwned::FloatingPoint(number)) => {
            if number.0.fract() == 0.0 {
                format!("{:.0}", number.0)
            } else {
                number.0.to_string()
            }
        }
        YamlOwned::Representation(text, ..) => text.clone(),
        _ => String::new(),
    }
}

/// saphyr 解析整段 frontmatter；非映射、空文档、语法错误一律回空表（TS 的 `try/catch`）。
fn yaml_values(frontmatter: &str) -> Map<String, Value> {
    let Ok(mut documents) = YamlOwned::load_from_str(frontmatter) else {
        return Map::new();
    };
    let Some(document) = documents.pop() else {
        return Map::new();
    };
    match node_to_json(&document) {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

/// TS `subagentMarkdown.ts` 的局部 `parseLooseFrontmatter`：只回值，不追踪裸值/非法嵌套。
/// 与 `frontmatter.rs` 的共享版本有一处差别：`mcpServers` 的列表项按**普通标量**解析
/// （共享版本用 mcp 专用解析）——这一步在合并里被 YAML 数组覆盖时不可见。
fn loose_values(frontmatter: &str) -> Map<String, Value> {
    let mut values = Map::new();
    let mut pending: Option<String> = None;
    for raw_line in frontmatter.split('\n') {
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(item) = list_item(line)
            && let Some(key) = pending.clone()
        {
            let mut existing: Vec<Value> = values
                .get(&key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            existing.push(parse_scalar_value(&item, false));
            values.insert(key, Value::Array(existing));
            continue;
        }
        pending = None;
        let Some((key, raw_value)) = key_value(line) else {
            continue;
        };
        if raw_value.trim().is_empty() {
            values.insert(key.clone(), Value::Array(Vec::new()));
            pending = Some(key);
            continue;
        }
        values.insert(key, parse_scalar_value(&raw_value, false));
    }
    values
}

fn list_item(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('-')?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let item = rest.trim_start();
    (!item.is_empty()).then(|| item.to_owned())
}

fn key_value(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' || bytes[index] == b'-')
    {
        index += 1;
    }
    let rest = line[index..]
        .trim_start_matches([' ', '\t'])
        .strip_prefix(':')?;
    Some((
        line[..index].to_owned(),
        rest.trim_start_matches([' ', '\t']).to_owned(),
    ))
}

/// TS `parseRuntimeCompatibleFrontmatter`。
pub fn parse_runtime_compatible_frontmatter(frontmatter: &str) -> Map<String, Value> {
    let from_yaml = yaml_values(frontmatter);
    let mut merged = from_yaml.clone();
    for (key, value) in loose_values(frontmatter) {
        merged.insert(key, value);
    }
    // `mcpServers` 在 YAML 解析成数组时以 YAML 为准（宽松版可能把它读成裸值空数组）。
    if let Some(Value::Array(items)) = from_yaml.get("mcpServers") {
        merged.insert("mcpServers".to_owned(), Value::Array(items.clone()));
    }
    merged
}
