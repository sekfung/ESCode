//! registry 从 settings 对象读取数组字段（spec §9.2：`null`/缺失等价空数组）。

use anyhow::{Context, Result};
use serde_json::Value;

pub(super) fn parse_array<T>(settings: &Value, key: &str) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    match settings.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => {
            serde_json::from_value(value.clone()).with_context(|| format!("settings.{key}"))
        }
    }
}
