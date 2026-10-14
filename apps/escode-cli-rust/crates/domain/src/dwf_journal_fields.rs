//! journal 的物理状态映射与 JSON 收窄（从 dwf_journal.rs 拆出）。

use crate::json_order::Json;

const PHYSICAL: [&str; 5] = ["pending", "running", "completed", "failed", "cancelled"];

pub(super) fn physical(status: &str) -> &'static str {
    PHYSICAL
        .iter()
        .copied()
        .find(|candidate| *candidate == status)
        .unwrap_or("pending")
}

/// JS `Number.isInteger` 语义（`Json` 只有 compact/parse，读数字要自己收窄）。
pub(super) fn as_int(value: &Json) -> Option<i64> {
    let Json::Number(number) = value else {
        return None;
    };
    number.as_i64().or_else(|| {
        number
            .as_f64()
            .filter(|float| float.is_finite() && float.fract() == 0.0)
            .map(|float| float as i64)
    })
}

pub(super) fn is_primary(value: &Json) -> bool {
    value.get("primary") == Some(&Json::Bool(true))
}

pub(super) fn string_field(value: &Json, key: &str) -> Option<String> {
    value.get(key).and_then(Json::as_str).map(str::to_owned)
}
