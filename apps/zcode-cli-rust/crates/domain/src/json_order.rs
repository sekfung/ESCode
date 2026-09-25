//! 保持键顺序的 JSON（serde_json 默认按键排序）。用于必须与 TS `JSON.stringify` 字节一致的场景：
//! 工具 input 转写、官方插件缓存的 marker / marketplace / plugin.json（docs/specs/rust-official-plugin-seed.md）。

use std::fmt::Write as _;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// JS `JSON.parse`：重复键保留首次位置、最后一次的值。
    pub fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn object() -> Self {
        Self::Object(Vec::new())
    }

    pub fn str(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Self::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// JS 赋值语义：已有键原位替换，新键追加到末尾。非对象时不做任何事。
    pub fn set(&mut self, key: &str, value: Json) {
        if let Self::Object(entries) = self {
            match entries.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => entries.push((key.to_owned(), value)),
            }
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    /// `JSON.stringify(value)`。
    pub fn compact(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None, 0);
        out
    }

    /// `JSON.stringify(value, null, 2)`。
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(2), 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        let newline = |out: &mut String, depth: usize| {
            if let Some(width) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(width * depth));
            }
        };
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(value) => write_number(out, value),
            Self::String(value) => out.push_str(&quote(value)),
            Self::Array(items) if items.is_empty() => out.push_str("[]"),
            Self::Object(entries) if entries.is_empty() => out.push_str("{}"),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    item.write(out, indent, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    newline(out, depth + 1);
                    out.push_str(&quote(key));
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    value.write(out, indent, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// JS 数字输出：整数值的浮点数不带小数点。
fn write_number(out: &mut String, value: &serde_json::Number) {
    match value.as_f64() {
        Some(f) if !value.is_i64() && !value.is_u64() && f.fract() == 0.0 && f.abs() < 1e21 => {
            let _ = write!(out, "{}", f as i64);
        }
        _ => {
            let _ = write!(out, "{value}");
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Json;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Json, E> {
                Ok(serde_json::Number::from_f64(v).map_or(Json::Null, Json::Number))
            }
            fn visit_str<E>(self, v: &str) -> Result<Json, E> {
                Ok(Json::String(v.to_owned()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut object = Json::object();
                while let Some((key, value)) = map.next_entry::<String, Json>()? {
                    object.set(&key, value);
                }
                Ok(object)
            }
        }
        deserializer.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_json_stringify_layout() {
        let value =
            Json::parse(r#"{"b":1,"a":[1,{"x":null}],"e":{},"f":[],"g":2.0,"b":3}"#).unwrap();
        assert_eq!(
            value.compact(),
            r#"{"b":3,"a":[1,{"x":null}],"e":{},"f":[],"g":2}"#
        );
        assert_eq!(
            value.pretty(),
            "{\n  \"b\": 3,\n  \"a\": [\n    1,\n    {\n      \"x\": null\n    }\n  ],\n  \"e\": {},\n  \"f\": [],\n  \"g\": 2\n}"
        );
    }
}
