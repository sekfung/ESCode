//! frontmatter 的标量/列表解析辅助（TS `subagent-profile-frontmatter.ts` 的 `parseScalarValue` 一族）。
//!
//! 与「切分 + 宽松结构解析」分开：这里只处理**值**——行内注释、引号、内联列表/映射、数值与 mcpServers 列表项。

use serde_json::{Number, Value};

pub(super) fn strip_inline_comment(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut quote: Option<char> = None;
    for index in 0..chars.len() {
        let character = chars[index];
        if (character == '"' || character == '\'')
            && chars.get(index.wrapping_sub(1)) != Some(&'\\')
        {
            quote = match quote {
                Some(open) if open == character => None,
                Some(open) => Some(open),
                None => Some(character),
            };
        }
        if quote.is_none()
            && character == '#'
            && chars
                .get(index.wrapping_sub(1))
                .is_some_and(|previous| previous.is_whitespace())
        {
            return value
                .chars()
                .take(index)
                .collect::<String>()
                .trim_end()
                .to_owned();
        }
    }
    value.to_owned()
}

pub(super) fn unquote_scalar(value: &str) -> String {
    let quoted = (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
        || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2);
    if quoted {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

pub(super) fn is_unquoted_mapping_value(raw_value: &str) -> bool {
    let value = strip_inline_comment(raw_value.trim());
    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    if quoted {
        return false;
    }
    (value.starts_with('{') && value.ends_with('}')) || colon_prefixed(&value)
}

/// `/^[^:]+:(?:\s|$)/`：冒号前至少一个非冒号字符，且后面是空白或行尾。
fn colon_prefixed(value: &str) -> bool {
    let Some(index) = value.find(':') else {
        return false;
    };
    if index == 0 {
        return false;
    }
    value[index + 1..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
}

pub(super) fn has_unquoted_inline_mapping_item(raw_value: &str) -> bool {
    let value = strip_inline_comment(raw_value.trim());
    if !(value.starts_with('[') && value.ends_with(']')) || value.len() < 2 {
        return false;
    }
    split_top_level_list(&value[1..value.len() - 1], true, false)
        .iter()
        .any(|item| is_unquoted_mapping_value(item))
}

pub(super) fn parse_mcp_server_list_item(raw_value: &str) -> Value {
    let value = strip_inline_comment(raw_value.trim());
    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    if quoted {
        return Value::from(unquote_scalar(&value));
    }
    if value.starts_with('[') && value.ends_with(']') {
        return Value::Null;
    }
    let lower = value.to_ascii_lowercase();
    if lower == "null" || lower == "~" {
        return Value::Null;
    }
    if lower == "true" || lower == "false" {
        return Value::Bool(lower == "true");
    }
    if is_number_literal(&value) {
        return parse_number(&value);
    }
    Value::from(value)
}

fn is_number_literal(value: &str) -> bool {
    let body = value.strip_prefix(['+', '-']).unwrap_or(value);
    if body.is_empty() {
        return false;
    }
    // `\d+(\.\d*)?|\.\d+` 后可选指数段。
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (body, None),
    };
    let mantissa_ok = {
        let (integer, fraction) = match mantissa.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (mantissa, None),
        };
        let digits_or_empty = |text: &str| text.chars().all(|c| c.is_ascii_digit());
        match fraction {
            None => !integer.is_empty() && digits_or_empty(integer),
            Some(fraction) if integer.is_empty() => {
                !fraction.is_empty() && digits_or_empty(fraction)
            }
            Some(fraction) => digits_or_empty(integer) && digits_or_empty(fraction),
        }
    };
    if !mantissa_ok {
        return false;
    }
    match exponent {
        None => true,
        Some(exponent) => {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
    }
}

fn parse_number(value: &str) -> Value {
    // JS 的 `Number("42")` 序列化回 `42`（不是 `42.0`）：整数走 i64，其余才用 f64。
    if !value.contains(['.', 'e', 'E'])
        && let Ok(integer) = value.parse::<i64>()
    {
        return Value::from(integer);
    }
    value
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map_or_else(|| Value::from(value.to_owned()), Value::Number)
}

pub(super) fn parse_scalar_value(raw_value: &str, parse_inline_item_scalars: bool) -> Value {
    let value = strip_inline_comment(raw_value.trim());
    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
        return parse_number(&value);
    }
    if value.starts_with('[') && value.ends_with(']') && value.len() >= 2 {
        let items = split_top_level_list(
            &value[1..value.len() - 1],
            parse_inline_item_scalars,
            parse_inline_item_scalars,
        );
        return Value::Array(
            items
                .into_iter()
                .map(|item| {
                    if parse_inline_item_scalars {
                        parse_mcp_server_list_item(&item)
                    } else {
                        Value::from(unquote_scalar(strip_inline_comment(item.trim()).as_str()))
                    }
                })
                .collect(),
        );
    }
    if value.starts_with('{') && value.ends_with('}') {
        return serde_json::from_str::<Value>(&value).unwrap_or_else(|_| Value::from(value));
    }
    Value::from(unquote_scalar(&value))
}

fn split_top_level_list(
    value: &str,
    track_container_depth: bool,
    preserve_empty_items: bool,
) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let (mut brace, mut bracket, mut paren) = (0_i32, 0_i32, 0_i32);
    for character in value.chars() {
        if (character == '"' || character == '\'') && quote.is_none() {
            quote = Some(character);
            current.push(character);
            continue;
        }
        if quote == Some(character) {
            quote = None;
            current.push(character);
            continue;
        }
        if quote.is_none() {
            match character {
                '(' => paren += 1,
                ')' => paren = (paren - 1).max(0),
                '{' if track_container_depth => brace += 1,
                '}' if track_container_depth => brace = (brace - 1).max(0),
                '[' if track_container_depth => bracket += 1,
                ']' if track_container_depth => bracket = (bracket - 1).max(0),
                _ => {}
            }
            if paren == 0 && brace == 0 && bracket == 0 && character == ',' {
                if !current.trim().is_empty() || preserve_empty_items {
                    items.push(current.trim().to_owned());
                }
                current.clear();
                continue;
            }
        }
        current.push(character);
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_owned());
    }
    items
}
