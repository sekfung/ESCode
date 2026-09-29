//! 已保存工作流文件（`.dwf.ts`）的编解码，逐条对齐 TS `saved-workflows/frontmatter.ts` 与
//! `contracts/src/tools/saved-workflow.ts` 的 `SavedWorkflowMetaSchema`（docs/specs/rust-dynamic-workflow.md 第 2 期）。
//!
//! 文件形状：`/* zcode-workflow` 起始行、YAML 元数据、`*/` 终止行，其后是逐字节保留的脚本。

use crate::json_order::Json;
use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};

pub const SENTINEL: &str = "/* zcode-workflow";
const TERMINATOR: &str = "*/";
const ARG_TYPES: [&str; 4] = ["string", "number", "boolean", "json"];
/// 私用区占位符：代替 `\r` 交给 saphyr（见 `parse`）。
const CR_PLACEHOLDER: char = '\u{E000}';

#[derive(Debug, PartialEq)]
pub struct Parsed {
    pub meta: Json,
    pub script: String,
    pub body_line_offset: usize,
}

#[derive(Debug, PartialEq)]
pub struct Failure {
    pub reason: &'static str,
    pub detail: String,
}

/// 元数据 + 脚本 → 文件正文。键序固定为 description → whenToUse → args（保存两次逐字节相同）。
pub fn serialize(meta: &Json, script: &str) -> String {
    let mut body = Json::object();
    for key in ["description", "whenToUse", "args"] {
        if let Some(value) = meta.get(key) {
            body.set(key, value.clone());
        }
    }
    format!("{SENTINEL}\n{}{TERMINATOR}\n{script}", crate::yaml_emit::stringify(&body))
}

/// 文件正文 → 元数据 + 脚本。四种失败各有原因：缺起始块、未闭合、YAML 坏、元数据不符。
pub fn parse(source: &str) -> Result<Parsed, Failure> {
    let lines: Vec<&str> = source.split('\n').collect();
    let blank = |l: &str| crate::web_fetch::js_trim(l).is_empty();
    let start = lines.iter().position(|l| !blank(l));
    let Some(start) = start.filter(|&s| crate::web_fetch::js_trim(lines[s]) == SENTINEL) else {
        return Err(Failure {
            reason: "missing_frontmatter",
            detail: format!("file does not start with the `{SENTINEL}` metadata block"),
        });
    };
    let Some(end) = (start + 1..lines.len()).find(|&i| crate::web_fetch::js_trim(lines[i]) == TERMINATOR) else {
        return Err(Failure {
            reason: "unterminated_frontmatter",
            detail: format!("metadata block is never closed with `{TERMINATOR}`"),
        });
    };
    let body = lines[start + 1..end].join("\n");
    let script = lines[end + 1..].join("\n");
    // TS 按 `\n` 切行，CRLF 文件每行残留的 `\r` 被 `yaml` 当作标量内容（如 description 为 "CRLF\r"）；
    // saphyr 把 `\r` 当换行，这里先换成私用区占位符、解析后还原，与 TS 结果一致。
    let body = body.replace('\r', &CR_PLACEHOLDER.to_string());
    let documents = YamlOwned::load_from_str(&body)
        .map_err(|error| Failure { reason: "invalid_yaml", detail: error.to_string() })?;
    let meta = documents.first().map(to_json).unwrap_or(Json::Null);
    let issues = validate(&meta);
    if !issues.is_empty() {
        return Err(Failure { reason: "invalid_metadata", detail: issues.join("; ") });
    }
    Ok(Parsed { meta: shape_order(&meta), script, body_line_offset: end + 1 })
}

/// zod 解析结果按 schema 字段顺序重建对象（元数据 description → whenToUse → args；
/// 参数 type → description → required → default），参数名保持文件顺序。
fn shape_order(meta: &Json) -> Json {
    let pick = |source: &Json, keys: &[&str]| {
        let mut out = Json::object();
        for key in keys {
            if let Some(value) = source.get(key) {
                out.set(key, value.clone());
            }
        }
        out
    };
    let mut out = pick(meta, &["description", "whenToUse"]);
    if let Some(Json::Object(args)) = meta.get("args") {
        let ordered = args
            .iter()
            .map(|(name, decl)| (name.clone(), pick(decl, &["type", "description", "required", "default"])))
            .collect();
        out.set("args", Json::Object(ordered));
    }
    out
}

fn to_json(node: &YamlOwned) -> Json {
    match node {
        YamlOwned::Value(scalar) => scalar_json(scalar),
        YamlOwned::Representation(text, ..) => Json::String(text.replace(CR_PLACEHOLDER, "\r")),
        YamlOwned::Sequence(items) => Json::Array(items.iter().map(to_json).collect()),
        YamlOwned::Mapping(map) => Json::Object(
            map.iter()
                .map(|(k, v)| {
                    // TS `yaml` 把非字符串键转成字符串键（JS 对象键）。
                    let key = match to_json(k) {
                        Json::String(s) => s,
                        Json::Null => String::new(),
                        other => other.compact().trim_matches('"').to_owned(),
                    };
                    (key, to_json(v))
                })
                .collect(),
        ),
        YamlOwned::Tagged(_, inner) => to_json(inner),
        _ => Json::Null,
    }
}

fn scalar_json(scalar: &ScalarOwned) -> Json {
    match scalar {
        ScalarOwned::Null => Json::Null,
        ScalarOwned::Boolean(b) => Json::Bool(*b),
        ScalarOwned::Integer(i) => Json::Number((*i).into()),
        ScalarOwned::FloatingPoint(f) => serde_json::Number::from_f64(f.0).map(Json::Number).unwrap_or(Json::Null),
        ScalarOwned::String(s) => Json::String(s.replace(CR_PLACEHOLDER, "\r")),
    }
}

/// zod 的类型名（`Expected X, received Y` 的 Y）。
fn kind(value: Option<&Json>) -> &'static str {
    match value {
        None => "undefined",
        Some(Json::Null) => "null",
        Some(Json::Bool(_)) => "boolean",
        Some(Json::Number(_)) => "number",
        Some(Json::String(_)) => "string",
        Some(Json::Array(_)) => "array",
        Some(Json::Object(_)) => "object",
    }
}

fn at(path: &str) -> String {
    if path.is_empty() { "(root)".into() } else { path.into() }
}

/// 一个 strict 对象里未声明的键（zod 在该对象的已知字段之后报一条）。
fn unknown_keys(entries: &[(String, Json)], known: &[&str]) -> Option<String> {
    let unknown: Vec<String> =
        entries.iter().filter(|(k, _)| !known.contains(&k.as_str())).map(|(k, _)| format!("'{k}'")).collect();
    (!unknown.is_empty()).then(|| format!("Unrecognized key(s) in object: {}", unknown.join(", ")))
}

/// `SavedWorkflowMetaSchema`（strict）：按 zod 遍历顺序给出 `path: message` 列表。
pub fn validate(meta: &Json) -> Vec<String> {
    let mut issues = Vec::new();
    let Json::Object(entries) = meta else {
        return vec![format!("(root): Expected object, received {}", kind(Some(meta)))];
    };
    let text = |issues: &mut Vec<String>, key: &str, required: bool| match meta.get(key) {
        None if !required => {}
        Some(Json::String(s)) if s.is_empty() => {
            issues.push(format!("{key}: String must contain at least 1 character(s)"))
        }
        Some(Json::String(_)) => {}
        None => issues.push(format!("{key}: Required")),
        other => issues.push(format!("{key}: Expected string, received {}", kind(other))),
    };
    text(&mut issues, "description", true);
    text(&mut issues, "whenToUse", false);
    match meta.get("args") {
        None => {}
        Some(Json::Object(args)) => {
            for (name, decl) in args {
                let path = format!("args.{name}");
                let Json::Object(fields) = decl else {
                    issues.push(format!("{path}: Expected object, received {}", kind(Some(decl))));
                    continue;
                };
                match decl.get("type") {
                    None => issues.push(format!("{path}.type: Required")),
                    Some(Json::String(t)) if ARG_TYPES.contains(&t.as_str()) => {}
                    Some(Json::String(t)) => issues.push(format!(
                        "{path}.type: Invalid enum value. Expected 'string' | 'number' | 'boolean' | 'json', received '{t}'"
                    )),
                    other => issues.push(format!(
                        "{path}.type: Expected 'string' | 'number' | 'boolean' | 'json', received {}",
                        kind(other)
                    )),
                }
                if let Some(value) = decl.get("description").filter(|v| !matches!(v, Json::String(_))) {
                    issues.push(format!("{path}.description: Expected string, received {}", kind(Some(value))));
                }
                if let Some(value) = decl.get("required").filter(|v| !matches!(v, Json::Bool(_))) {
                    issues.push(format!("{path}.required: Expected boolean, received {}", kind(Some(value))));
                }
                if let Some(message) = unknown_keys(fields, &["type", "description", "required", "default"]) {
                    issues.push(format!("{}: {message}", at(&path)));
                }
            }
        }
        other => issues.push(format!("args: Expected object, received {}", kind(other))),
    }
    if let Some(message) = unknown_keys(entries, &["description", "whenToUse", "args"]) {
        issues.push(format!("(root): {message}"));
    }
    issues
}

#[cfg(test)]
#[path = "saved_workflow_tests.rs"]
mod tests;
