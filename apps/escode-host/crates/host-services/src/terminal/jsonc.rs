//! JSONC 宽松解析（TS `terminalProfile.ts` 的 `stripJsonComments` / `removeJsonTrailingCommas` / `parseJsonc`）。

use serde_json::{Map, Value};

/// JS `\s`（含 BOM、Unicode 空白；不含 U+0085）。
pub fn is_js_space(character: char) -> bool {
    character == '\u{feff}' || (character.is_whitespace() && character != '\u{85}')
}

/// TS `stripJsonComments`。
fn strip_json_comments(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut result = String::with_capacity(raw.len());
    let (mut in_string, mut escaped) = (false, false);
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if in_string {
            result.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
        } else if character == '"' {
            in_string = true;
            result.push(character);
        } else if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            result.push('\n');
        } else if character == '/' && next == Some('*') {
            index += 2;
            while index < chars.len()
                && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
            {
                index += 1;
            }
            index += 1;
        } else {
            result.push(character);
        }
        index += 1;
    }
    result
}

/// TS `removeJsonTrailingCommas`。
fn remove_json_trailing_commas(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut result = String::with_capacity(raw.len());
    let (mut in_string, mut escaped) = (false, false);
    for (index, &character) in chars.iter().enumerate() {
        if in_string {
            result.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
        }
        if character == ',' {
            let following = chars[index + 1..].iter().find(|c| !is_js_space(**c));
            if matches!(following, Some('}' | ']')) {
                continue;
            }
        }
        result.push(character);
    }
    result
}

/// TS `parseJsonc`：先原样解析，失败再去注释/尾逗号；结果须是对象。
pub fn parse_jsonc(raw: &str) -> Option<Map<String, Value>> {
    let relaxed = remove_json_trailing_commas(&strip_json_comments(raw));
    [raw, relaxed.as_str()].into_iter().find_map(|candidate| {
        match serde_json::from_str::<Value>(candidate) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        }
    })
}
