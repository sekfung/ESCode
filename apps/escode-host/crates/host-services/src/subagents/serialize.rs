//! agent markdown 的序列化（TS `subagentMarkdown.ts` 的 `serializeSubagentMarkdown`）。
//!
//! frontmatter 行序与 TS 逐行一致；标量按 `formatYamlScalar` 决定是否加引号
//! （`*`、含 `: `、`#`、看起来像数字/布尔/空值的都要引），`name`/`description` **总是**引号+转义。
//!
//! **登记差异**：非字符串数组的 `mcpServers` 由 TS 的 `yaml.stringify` 输出；这里用等价的
//! 块状 emitter（标量/数组/对象逐层缩进 2 空格），不复制 `yaml` 的引号风格、锚点与流式写法。
//! 只在 `mcpServers` 含对象/嵌套结构且值需要特殊引号时可能逐字不同（语义等价）。

use serde_json::{Map, Value};

/// JS `encodeURIComponent`：保留 `A-Za-z0-9-_.!~*'()`，其余按 UTF-8 百分号编码。
fn encode_uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// TS `formatSubagentMarkdownModel`：`custom:` 前缀、provider 含 `/`、model 含 `$` 时走编码。
pub fn format_model(selection: &Value) -> String {
    let provider = selection["providerId"].as_str().unwrap_or_default();
    let model = selection["modelId"].as_str().unwrap_or_default();
    if provider.starts_with("custom:") || provider.contains('/') || model.contains('$') {
        format!(
            "custom:{}:{}",
            encode_uri_component(provider),
            encode_uri_component(model)
        )
    } else {
        format!("{provider}/{model}")
    }
}

fn escape_yaml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// `\d+` 或 `\d*\.\d+`（可带指数）。
fn is_numeric(value: &str) -> bool {
    let body = value.strip_prefix(['+', '-']).unwrap_or(value);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (body, None),
    };
    let digits = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        None => digits(mantissa),
        Some(("", fraction)) => digits(fraction),
        Some((integer, fraction)) => digits(integer) && digits(fraction),
    };
    mantissa_ok
        && exponent
            .is_none_or(|exponent| digits(exponent.strip_prefix(['+', '-']).unwrap_or(exponent)))
}

/// TS `isSafePlainYamlScalar`：不安全的一律加引号。
fn is_safe_plain(value: &str) -> bool {
    if value.is_empty() || value.trim() != value {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "false" | "null" | "true" | "~") || is_numeric(value) {
        return false;
    }
    if value.starts_with([
        '*', '?', ':', ',', '[', ']', '{', '}', '&', '!', '|', '>', '\'', '"', '%', '@', '`', '-',
    ]) {
        return false;
    }
    if value.contains('#') || value.contains(": ") {
        return false;
    }
    // `^[A-Za-z0-9_./@*][A-Za-z0-9_./@*\s()-]*$`
    value.chars().enumerate().all(|(index, character)| {
        let allowed = character.is_ascii_alphanumeric()
            || matches!(character, '_' | '.' | '/' | '@' | '*' | '-' | '(' | ')')
            || character.is_whitespace();
        allowed
            && (index > 0
                || character.is_ascii_alphanumeric()
                || matches!(character, '_' | '.' | '/' | '@' | '*'))
    })
}

fn format_yaml_scalar(value: &str) -> String {
    if is_safe_plain(value) {
        value.to_owned()
    } else {
        format!("\"{}\"", escape_yaml_string(value))
    }
}

/// `yaml.stringify` 的 plain-scalar 判定（比手写行的 `formatYamlScalar` 更宽松：
/// `--foo`、`a:b` 这类都能裸写，只有首字符是指标且后跟空白/结束、或含 `: `、` #` 才要引号）。
fn is_plain_yaml(value: &str) -> bool {
    if value.is_empty() || value.trim() != value {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "false" | "null" | "true" | "~") || is_numeric(value) {
        return false;
    }
    let mut characters = value.chars();
    let first = characters.next().unwrap_or(' ');
    if matches!(first, '-' | '?' | ':') {
        match characters.next() {
            None | Some(' ') => return false,
            _ => {}
        }
    } else if matches!(
        first,
        ',' | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    ) {
        return false;
    }
    !(value.contains(": ") || value.contains(" #"))
}

fn append_scalar(lines: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        lines.push(format!("{key}: {}", format_yaml_scalar(value)));
    }
}

fn append_list(lines: &mut Vec<String>, key: &str, values: Option<&Value>) {
    let Some(items) = values
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
    else {
        return;
    };
    lines.push(format!("{key}:"));
    for item in items {
        let text = item.as_str().unwrap_or_default();
        lines.push(format!("  - {}", format_yaml_scalar(text)));
    }
}

fn is_string_array(items: &[Value]) -> bool {
    items.iter().all(Value::is_string)
}

/// 块状 emitter（`yaml.stringify` 的等价子集）：对象按 `key:` 展开，数组按 `- ` 展开。
fn emit_block(lines: &mut Vec<String>, indent: usize, value: &Value) {
    let padding = " ".repeat(indent);
    match value {
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                        lines.push(format!("{padding}- {}", scalar_text(item)));
                    }
                    _ => {
                        lines.push(format!("{padding}-"));
                        emit_block(lines, indent + 2, item);
                    }
                }
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                match value {
                    Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
                        lines.push(format!("{padding}{key}: {}", scalar_text(value)));
                    }
                    _ => {
                        lines.push(format!("{padding}{key}:"));
                        emit_block(lines, indent + 2, value);
                    }
                }
            }
        }
        _ => {}
    }
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) if is_plain_yaml(text) => text.clone(),
        Value::String(text) => format!("\"{}\"", escape_yaml_string(text)),
        other => other.to_string(),
    }
}

fn append_unknown_list(lines: &mut Vec<String>, key: &str, values: Option<&Value>) {
    let Some(items) = values
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
    else {
        return;
    };
    if is_string_array(items) {
        append_list(lines, key, Some(&Value::Array(items.clone())));
        return;
    }
    // 对象数组必须按结构写回，避免编辑其他字段时清空 MCP 配置。
    lines.push(format!("{key}:"));
    emit_block(lines, 2, &Value::Array(items.clone()));
}

fn format_body(system_prompt: &str) -> String {
    let body = system_prompt.trim();
    if body.is_empty() {
        "\n".to_owned()
    } else {
        format!("\n{body}\n")
    }
}

/// TS `serializeSubagentMarkdown`。
pub fn serialize_subagent_markdown(config: &Map<String, Value>) -> String {
    let text = |key: &str| config.get(key).and_then(Value::as_str).unwrap_or_default();
    let mut lines = vec![
        format!("name: \"{}\"", escape_yaml_string(text("name"))),
        format!(
            "description: \"{}\"",
            escape_yaml_string(text("description"))
        ),
    ];
    append_scalar(
        &mut lines,
        "color",
        config.get("color").and_then(Value::as_str),
    );
    if let Some(selection) = config
        .get("modelSelection")
        .filter(|value| value.is_object())
    {
        append_scalar(&mut lines, "model", Some(&format_model(selection)));
        append_scalar(
            &mut lines,
            "thoughtLevel",
            selection["options"]["reasoningLevel"].as_str(),
        );
    }
    append_list(&mut lines, "tools", config.get("tools"));
    append_list(&mut lines, "disallowedTools", config.get("disallowedTools"));
    append_list(&mut lines, "skills", config.get("skills"));
    append_scalar(
        &mut lines,
        "permissionMode",
        config.get("permissionMode").and_then(Value::as_str),
    );
    if let Some(turns) = config.get("maxTurns").filter(|value| !value.is_null()) {
        lines.push(format!("maxTurns: {}", scalar_text(turns)));
    }
    for (key, field) in [
        ("background", "background"),
        ("injectAgentsMd", "injectAgentsMd"),
    ] {
        if let Some(flag) = config.get(field).and_then(Value::as_bool) {
            lines.push(format!("{key}: {flag}"));
        }
    }
    append_unknown_list(&mut lines, "mcpServers", config.get("mcpServers"));

    format!(
        "---\n{}\n---\n{}",
        lines.join("\n"),
        format_body(text("systemPrompt"))
    )
}

#[cfg(test)]
#[path = "serialize_tests.rs"]
mod tests;
