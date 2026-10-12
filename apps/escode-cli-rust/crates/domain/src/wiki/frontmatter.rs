//! 条目文件编解码：`---` YAML frontmatter + Markdown 正文。
//! 解析复用 saphyr（YAML 1.2 core schema），序列化复用 `yaml_emit`（与已保存工作流同一套规则）；
//! frontmatter 键序固定为 `Meta` 字段顺序，同一条目两次写出逐字节相同（Git 友好）。
use super::entry::{Entry, Meta};
use crate::json_order::Json;
use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};
use serde_json::Value;

const FENCE: &str = "---";

#[derive(Debug, PartialEq)]
pub enum ParseError {
    MissingFrontmatter,
    Unterminated,
    InvalidYaml(String),
    InvalidMeta(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "missing `---` frontmatter"),
            Self::Unterminated => write!(f, "frontmatter is not closed with `---`"),
            Self::InvalidYaml(e) => write!(f, "invalid frontmatter YAML: {e}"),
            Self::InvalidMeta(e) => write!(f, "invalid entry metadata: {e}"),
        }
    }
}

/// 切出 frontmatter 文本与正文；正文去掉一个前导空行。
pub fn split(source: &str) -> Result<(&str, &str), ParseError> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let rest = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))
        .ok_or(ParseError::MissingFrontmatter)?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == FENCE {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            let body = body
                .strip_prefix("\r\n")
                .or_else(|| body.strip_prefix('\n'))
                .unwrap_or(body);
            return Ok((yaml, body));
        }
        offset += line.len();
    }
    Err(ParseError::Unterminated)
}

/// YAML 文本 → serde_json::Value（第一个文档）。
pub fn yaml_value(text: &str) -> Result<Value, ParseError> {
    let documents =
        YamlOwned::load_from_str(text).map_err(|e| ParseError::InvalidYaml(e.to_string()))?;
    Ok(documents.first().map(yaml_to_value).unwrap_or(Value::Null))
}

pub fn yaml_to_value(node: &YamlOwned) -> Value {
    match node {
        YamlOwned::Value(scalar) => match scalar {
            ScalarOwned::Null => Value::Null,
            ScalarOwned::Boolean(b) => Value::Bool(*b),
            ScalarOwned::Integer(i) => Value::from(*i),
            ScalarOwned::FloatingPoint(f) => serde_json::Number::from_f64(f.0)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            ScalarOwned::String(s) => Value::String(s.clone()),
        },
        YamlOwned::Representation(text, ..) => Value::String(text.clone()),
        YamlOwned::Sequence(items) => Value::Array(items.iter().map(yaml_to_value).collect()),
        YamlOwned::Mapping(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let key = match yaml_to_value(k) {
                        Value::String(s) => s,
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    (key, yaml_to_value(v))
                })
                .collect(),
        ),
        YamlOwned::Tagged(_, inner) => yaml_to_value(inner),
        _ => Value::Null,
    }
}

pub fn parse(source: &str) -> Result<Entry, ParseError> {
    let (yaml, body) = split(source)?;
    let value = yaml_value(yaml)?;
    let meta: Meta =
        serde_json::from_value(value).map_err(|e| ParseError::InvalidMeta(e.to_string()))?;
    Ok(Entry {
        meta,
        body: body.to_owned(),
    })
}

/// 仅解析 frontmatter（不要求合法条目）：导入 Markdown 时识别已有知识 ID 用。
pub fn parse_loose(source: &str) -> Option<(Value, &str)> {
    let (yaml, body) = split(source).ok()?;
    Some((yaml_value(yaml).ok()?, body))
}

pub fn serialize(entry: &Entry) -> String {
    // 结构体按字段顺序序列化，再用保序 JSON 交给 YAML 发射器，避免 serde_json Map 的键排序。
    let text = serde_json::to_string(&entry.meta).unwrap_or_else(|_| "{}".into());
    let meta = Json::parse(&text).unwrap_or_else(Json::object);
    let mut out = String::from("---\n");
    out.push_str(&crate::yaml_emit::stringify(&meta));
    out.push_str("---\n\n");
    out.push_str(entry.body.trim_end_matches('\n'));
    out.push('\n');
    out
}
