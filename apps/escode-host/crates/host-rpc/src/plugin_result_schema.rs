//! `plugins/list`、`plugins/overview` 的结果校验（docs/specs/rust-u2-plugin-management.md §7）。
//!
//! 对应 TS `shared/escode-protocol` 的 zod schema（4.6.5）。这些 schema 全是 `.strict()` 且没有 `default/transform`：
//! 所以这里只是**纯接受/拒绝**——成功时应答原样透传，与 TS `parse` 的结果逐值相同。差分见
//! `scripts/generate-escode-host-plugin-schema-fixtures.mjs`（真实 zod 对变异语料求 `safeParse().success`）。

use serde_json::Value;

/// 一个字段：`optional` = 键缺省可，但 `null` 不可（zod `.optional()` 不接受 `null`）。
pub(crate) struct Field {
    name: &'static str,
    ty: &'static Ty,
    optional: bool,
}

pub(crate) const fn req(name: &'static str, ty: &'static Ty) -> Field {
    Field {
        name,
        ty,
        optional: false,
    }
}

pub(crate) const fn opt(name: &'static str, ty: &'static Ty) -> Field {
    Field {
        name,
        ty,
        optional: true,
    }
}

pub(crate) enum Ty {
    /// `z.string()`
    Str,
    /// `z.string().trim().min(1)`（`trim` 用 JS 空白集）。
    NonEmpty,
    Bool,
    /// `z.literal(true)`
    True,
    /// `z.literal("…")`
    Lit(&'static str),
    Enum(&'static [&'static str]),
    /// `z.number()`（JSON 里只会是有限数）
    Num,
    /// `z.number().positive()`
    PosNum,
    /// `z.number().int().nonnegative()`
    NonNegInt,
    /// `z.number().int().positive()`
    PosInt,
    Arr(&'static Ty),
    /// `z.record(z.string(), T)`
    Rec(&'static Ty),
    /// `z.record(z.string(), z.unknown())`：任意对象（数组/`null` 都不是）。
    AnyObj,
    /// `z.object({…}).strict()`
    Obj(&'static [Field]),
    Union(&'static [&'static Ty]),
    /// 先过内层 schema，再过跨字段约束（zod `.superRefine`）；返回 `Err(原因)` 即拒绝。
    Check(&'static Ty, fn(&Value) -> Result<(), &'static str>),
}

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

fn safe_int(value: &Value) -> Option<f64> {
    let n = value.as_f64()?;
    (n.fract() == 0.0 && n.abs() <= MAX_SAFE).then_some(n)
}

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 校验 `value`；失败返回 `"<path>: <reason>"`（`path` 以 `.` 连接，数组下标用数字，根为 `result`）。
pub(crate) fn validate(ty: &Ty, value: &Value) -> Result<(), String> {
    check(ty, value, &mut vec!["result".to_owned()])
}

fn fail(path: &[String], reason: impl std::fmt::Display) -> Result<(), String> {
    Err(format!("{}: {reason}", path.join(".")))
}

fn check(ty: &Ty, value: &Value, path: &mut Vec<String>) -> Result<(), String> {
    match ty {
        Ty::Str => match value {
            Value::String(_) => Ok(()),
            other => fail(
                path,
                format!("expected string, received {}", describe(other)),
            ),
        },
        Ty::NonEmpty => match value {
            Value::String(text) if !escode_host_services::js_compat::js_trim(text).is_empty() => {
                Ok(())
            }
            Value::String(_) => fail(path, "string must contain at least 1 non-space character"),
            other => fail(
                path,
                format!("expected string, received {}", describe(other)),
            ),
        },
        Ty::Bool => match value {
            Value::Bool(_) => Ok(()),
            other => fail(
                path,
                format!("expected boolean, received {}", describe(other)),
            ),
        },
        Ty::True => match value {
            Value::Bool(true) => Ok(()),
            _ => fail(path, "expected literal true"),
        },
        Ty::Lit(expected) => match value {
            Value::String(text) if text == expected => Ok(()),
            _ => fail(path, format!("expected literal \"{expected}\"")),
        },
        Ty::Enum(allowed) => match value {
            Value::String(text) if allowed.contains(&text.as_str()) => Ok(()),
            _ => fail(path, format!("expected one of {}", allowed.join("|"))),
        },
        Ty::Num => match value {
            Value::Number(_) => Ok(()),
            other => fail(
                path,
                format!("expected number, received {}", describe(other)),
            ),
        },
        Ty::PosNum => match value.as_f64() {
            Some(n) if n > 0.0 => Ok(()),
            Some(_) => fail(path, "number must be greater than 0"),
            None => fail(
                path,
                format!("expected number, received {}", describe(value)),
            ),
        },
        Ty::NonNegInt => match (value.is_number(), safe_int(value)) {
            (true, Some(n)) if n >= 0.0 => Ok(()),
            (true, _) => fail(path, "expected a non-negative safe integer"),
            _ => fail(
                path,
                format!("expected number, received {}", describe(value)),
            ),
        },
        Ty::PosInt => match (value.is_number(), safe_int(value)) {
            (true, Some(n)) if n > 0.0 => Ok(()),
            (true, _) => fail(path, "expected a positive safe integer"),
            _ => fail(
                path,
                format!("expected number, received {}", describe(value)),
            ),
        },
        Ty::Arr(item) => {
            let Value::Array(items) = value else {
                return fail(
                    path,
                    format!("expected array, received {}", describe(value)),
                );
            };
            for (index, entry) in items.iter().enumerate() {
                path.push(index.to_string());
                check(item, entry, path)?;
                path.pop();
            }
            Ok(())
        }
        Ty::Rec(item) => {
            let Value::Object(map) = value else {
                return fail(
                    path,
                    format!("expected object, received {}", describe(value)),
                );
            };
            for (key, entry) in map {
                path.push(key.clone());
                check(item, entry, path)?;
                path.pop();
            }
            Ok(())
        }
        Ty::AnyObj => match value {
            Value::Object(_) => Ok(()),
            other => fail(
                path,
                format!("expected object, received {}", describe(other)),
            ),
        },
        Ty::Obj(fields) => {
            let Value::Object(map) = value else {
                return fail(
                    path,
                    format!("expected object, received {}", describe(value)),
                );
            };
            for key in map.keys() {
                if !fields.iter().any(|field| field.name == key) {
                    return fail(path, format!("unrecognized key \"{key}\""));
                }
            }
            for field in *fields {
                match map.get(field.name) {
                    None if field.optional => {}
                    None => {
                        path.push(field.name.to_owned());
                        let result = fail(path, "required");
                        path.pop();
                        return result;
                    }
                    Some(entry) => {
                        path.push(field.name.to_owned());
                        check(field.ty, entry, path)?;
                        path.pop();
                    }
                }
            }
            Ok(())
        }
        Ty::Check(inner, refine) => {
            check(inner, value, path)?;
            refine(value).or_else(|reason| fail(path, reason))
        }
        Ty::Union(options) => {
            if options
                .iter()
                .any(|option| check(option, value, &mut path.clone()).is_ok())
            {
                Ok(())
            } else {
                fail(path, "no union member matches")
            }
        }
    }
}

#[cfg(test)]
#[path = "plugin_result_schema_tests.rs"]
mod tests;
