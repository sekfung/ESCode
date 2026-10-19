//! 保序 JSON：hooks 配置写回要求与 TS 的 `JSON.parse`/对象展开同序。
//!
//! `serde_json::Value` 的 `Map` 是 BTreeMap（默认特性），解析后键序变为字典序；TS 用
//! `JSON.stringify(..., null, 2)` 保留插入顺序。持久化格式按 L5 对拍时，键序是可见字节，
//! 因此这里用 `IndexMap` 保留文档顺序，并用 `serde_json::to_string_pretty` 输出同一缩进。

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 递归 JSON 值，对象保留键序。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrderedJson {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<OrderedJson>),
    Object(IndexMap<String, OrderedJson>),
}

impl OrderedJson {
    pub fn as_object(&self) -> Option<&IndexMap<String, OrderedJson>> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }

    /// `serde_json::Value` → 保序值。`Value::Object` 的键序已在解析时丢失，
    /// 该转换只用于 Host 自己构造的新对象。
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(flag) => Self::Bool(*flag),
            Value::Number(number) => Self::Number(number.clone()),
            Value::String(text) => Self::String(text.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from_value).collect()),
            Value::Object(object) => Self::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_value(value)))
                    .collect(),
            ),
        }
    }

    pub fn into_value(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(flag) => Value::Bool(flag),
            Self::Number(number) => Value::Number(number),
            Self::String(text) => Value::String(text),
            Self::Array(items) => Value::Array(items.into_iter().map(Self::into_value).collect()),
            Self::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| (key, value.into_value()))
                    .collect(),
            ),
        }
    }
}

/// `JSON.stringify(value, null, 2) + "\n"`。
pub fn to_pretty(value: &OrderedJson) -> Result<String, serde_json::Error> {
    Ok(format!("{}\n", serde_json::to_string_pretty(value)?))
}

/// 解析配置文件；失败或根不是对象时返回 `None`（TS `readJsonFile` 的 `null`）。
pub fn parse_object(text: &str) -> Option<IndexMap<String, OrderedJson>> {
    match serde_json::from_str::<OrderedJson>(text).ok()? {
        OrderedJson::Object(object) => Some(object),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_objects_keep_insertion_order_and_typescript_indentation() {
        let parsed = parse_object(
            r#"{"features":{"mcp":true},"hooks":{"enabled":false,"events":{}},"n":1}"#,
        )
        .unwrap();
        let mut root: IndexMap<String, OrderedJson> = parsed;
        root.insert(
            "hooks".to_owned(),
            OrderedJson::Object(IndexMap::from([(
                "events".to_owned(),
                OrderedJson::Object(IndexMap::new()),
            )])),
        );
        let text = to_pretty(&OrderedJson::Object(root)).unwrap();
        assert_eq!(
            text,
            "{\n  \"features\": {\n    \"mcp\": true\n  },\n  \"hooks\": {\n    \"events\": {}\n  },\n  \"n\": 1\n}\n"
        );
    }

    #[test]
    fn non_object_roots_are_not_configs() {
        assert!(parse_object("[]").is_none());
        assert!(parse_object("null").is_none());
        assert!(parse_object("{broken").is_none());
    }

    #[test]
    fn round_trips_values() {
        let value = json!({"b": [1, 2.5, null, true, "x"], "a": {}});
        let ordered = OrderedJson::from_value(&value);
        assert_eq!(ordered.into_value(), value);
    }
}
