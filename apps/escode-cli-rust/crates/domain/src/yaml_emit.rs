//! 已保存工作流 frontmatter 的 YAML 序列化，逐字节对齐 TS `yaml` 库 `stringify` 的缺省行为
//! （docs/specs/rust-dynamic-workflow.md 第 2 期；以 TS oracle 语料验收）：
//! - 普通字符串走 plain，命中指示符、`: `、` #`、首尾空白或会被 core schema 解析成非字符串时用双引号；
//! - 含换行的字符串用块字面量（`|`/`|-`/`|+`）；
//! - plain 标量按 80 列贪心折行，续行缩进两格；
//! - 数字按 JS `Number#toString`。

use crate::json_order::Json;

#[allow(unused_imports)]
pub(super) use super::yaml_fold::{
    FoldMode, Folded, consume_more_indented_lines, fold_block, fold_flow_lines,
};

pub(crate) const LINE_WIDTH: usize = 80;

pub(crate) const MIN_CONTENT_WIDTH: usize = 20;

/// TS `doubleQuotedMinMultiLineLength`。
pub(crate) const DOUBLE_QUOTED_MIN_MULTI_LINE: usize = 40;

/// 顶层映射 → YAML 文本（带尾换行）。
pub fn stringify(value: &Json) -> String {
    let mut out = String::new();
    match value {
        Json::Object(entries) if !entries.is_empty() => write_mapping(&mut out, entries, 0),
        other => {
            out.push_str(&scalar(other, 0, 0));
            out.push('\n');
        }
    }
    out
}

fn write_mapping(out: &mut String, entries: &[(String, Json)], indent: usize) {
    for (key, value) in entries {
        let pad = " ".repeat(indent);
        let key_text = plain_or_quoted(key, true, 0, 0);
        out.push_str(&pad);
        out.push_str(&key_text);
        out.push(':');
        // TS `yaml` 的 indentAtStart 只算键与 `: `，不含父级缩进：嵌套值首行可超过 80 列（逐字对齐）。
        write_value(out, value, indent, key_text.chars().count() + 2);
    }
}

/// `column` 是值所在行已占用的列数（键与 `: `）；用于首行折行。
fn write_value(out: &mut String, value: &Json, indent: usize, column: usize) {
    match value {
        Json::Object(entries) if !entries.is_empty() => {
            out.push('\n');
            write_mapping(out, entries, indent + 2);
        }
        Json::Array(items) if !items.is_empty() => {
            out.push('\n');
            write_sequence(out, items, indent + 2);
        }
        other => {
            out.push(' ');
            out.push_str(&scalar(other, indent + 2, column));
            out.push('\n');
        }
    }
}

fn write_sequence(out: &mut String, items: &[Json], indent: usize) {
    for item in items {
        let pad = " ".repeat(indent);
        match item {
            // `- key: value` 紧凑写法：首个键跟在 `- ` 后面，其余键与之对齐。
            Json::Object(entries) if !entries.is_empty() => {
                let mut nested = String::new();
                write_mapping(&mut nested, entries, indent + 2);
                let body = nested
                    .strip_prefix(&" ".repeat(indent + 2))
                    .unwrap_or(&nested);
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(body);
            }
            Json::Array(inner) if !inner.is_empty() => {
                let mut nested = String::new();
                write_sequence(&mut nested, inner, indent + 2);
                let body = nested
                    .strip_prefix(&" ".repeat(indent + 2))
                    .unwrap_or(&nested);
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(body);
            }
            other => {
                out.push_str(&pad);
                out.push_str("- ");
                out.push_str(&scalar(other, indent + 2, indent + 2));
                out.push('\n');
            }
        }
    }
}

fn scalar(value: &Json, indent: usize, column: usize) -> String {
    match value {
        Json::Null => "null".into(),
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => yaml_number(n),
        Json::String(s) => string(s, indent, column),
        Json::Object(_) => "{}".into(),
        Json::Array(_) => "[]".into(),
    }
}

/// TS `stringifyNumber`：负零写 `-0`（`Object.is(value, -0)`，JSON 里 `-0` 会退化成 `0`），
/// 其余同 JS `Number#toString`。
fn yaml_number(value: &serde_json::Number) -> String {
    if let Some(f) = value.as_f64()
        && f == 0.0
        && f.is_sign_negative()
    {
        return "-0".into();
    }
    js_number(value)
}

fn string(value: &str, indent: usize, column: usize) -> String {
    // 值位置的多行字符串一律用块字面量（TS plainString / blockString）。
    if value.contains('\n') {
        // TS `blockString`：块标量不能以「换行 + 空白」结尾，命中时退回引号（quotedString）。
        if ends_with_newline_space(value) {
            return double_quoted(value, false, indent, column);
        }
        return block_literal(value, indent);
    }
    plain_or_quoted(value, false, indent, column)
}

/// TS `/\\n[\\t ]+$/`：末尾是换行后跟一段空白。
fn ends_with_newline_space(value: &str) -> bool {
    let trimmed = value.trim_end_matches([' ', '\t']);
    trimmed.len() != value.len() && trimmed.ends_with('\n')
}

/// TS `plainString` 的拒绝条件（首字符指示符、`: `、` #`、首尾空白、单独的 `-`/`?` 等）。
fn plain_blocked(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let chars: Vec<char> = value.chars().collect();
    let first = chars[0];
    if "\n\t ,[]{}#&*!|>'\"%@`".contains(first) {
        return true;
    }
    if (first == '-' || first == '?') && (chars.len() == 1 || matches!(chars[1], ' ' | '\t')) {
        return true;
    }
    for pair in chars.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if (matches!(a, '\n' | ':') && matches!(b, ' ' | '\t'))
            || (matches!(a, ' ' | '\t') && b == '\n')
            || (matches!(a, '\n' | '\t' | ' ') && b == '#')
        {
            return true;
        }
    }
    matches!(chars[chars.len() - 1], '\n' | '\t' | ' ' | ':')
        || value.starts_with("---")
        || value.starts_with("...")
}

/// plain 会被 core schema 解析成非字符串（null/bool/int/float）时必须加引号。
fn resolves_to_non_string(value: &str) -> bool {
    let re = |pattern: &str| regex::Regex::new(pattern).unwrap().is_match(value);
    re(r"^(?:~|[Nn]ull|NULL)?$")
        || re(r"^(?:[Tt]rue|TRUE|[Ff]alse|FALSE)$")
        || re(r"^[-+]?[0-9]+$")
        || re(r"^0o[0-7]+$")
        || re(r"^0x[0-9a-fA-F]+$")
        || re(r"^(?:[-+]?\.(?:inf|Inf|INF)|\.nan|\.NaN|\.NAN)$")
        || re(r"^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)[eE][-+]?[0-9]+$")
        || re(r"^[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*)$")
}

fn plain_or_quoted(value: &str, key: bool, indent: usize, column: usize) -> String {
    if plain_blocked(value) || resolves_to_non_string(value) {
        return double_quoted(value, key, indent, column);
    }
    if key {
        return value.to_owned();
    }
    fold(value, indent, column)
}

/// TS `foldFlowLines(text, indent, 'flow', { indentAtStart })` 的结果。
fn fold(value: &str, indent: usize, column: usize) -> String {
    fold_flow_lines(value, &" ".repeat(indent), FoldMode::Flow, Some(column)).text
}

/// TS `doubleQuotedString`：JSON 转义改写 + （非隐式键时）按 `'quoted'` 模式折行。
fn double_quoted(value: &str, implicit_key: bool, indent: usize, column: usize) -> String {
    let json = serde_json::to_string(value).unwrap_or_default();
    let text = rewrite_json_escapes(&json, indent);
    if implicit_key {
        // 隐式键不折行（TS `implicitKey ? str : foldFlowLines(...)`）。
        return text;
    }
    fold_flow_lines(&text, &" ".repeat(indent), FoldMode::Quoted, Some(column)).text
}

/// TS `doubleQuotedString` 的转义改写：命名的 C 风格转义（`\0`/`\a`/…）、`\u00xx` → `\xXX`，
/// 以及「换行前的空格」改写为 `\ `（否则会被折行吃掉），并按 `doubleQuotedMinMultiLineLength`
/// 决定嵌入换行是保持转义还是展开成多行。
fn rewrite_json_escapes(json: &str, indent: usize) -> String {
    let chars: Vec<char> = json.chars().collect();
    let at = |i: i64| -> Option<char> {
        if i < 0 {
            None
        } else {
            chars.get(i as usize).copied()
        }
    };
    let mut out = String::new();
    let mut start = 0usize;
    let mut i: i64 = 0;
    while let Some(mut ch) = at(i) {
        if ch == ' ' && at(i + 1) == Some('\\') && at(i + 2) == Some('n') {
            out.push_str(&chars[start..i as usize].iter().collect::<String>());
            out.push_str("\\ ");
            i += 1;
            start = i as usize;
            ch = '\\';
        }
        if ch == '\\' {
            match at(i + 1) {
                Some('u') => {
                    out.push_str(&chars[start..i as usize].iter().collect::<String>());
                    let code: String = chars[i as usize + 2..(i as usize + 6).min(chars.len())]
                        .iter()
                        .collect();
                    match code.as_str() {
                        "0000" => out.push_str("\\0"),
                        "0007" => out.push_str("\\a"),
                        "000b" => out.push_str("\\v"),
                        "001b" => out.push_str("\\e"),
                        "0085" => out.push_str("\\N"),
                        "00a0" => out.push_str("\\_"),
                        "2028" => out.push_str("\\L"),
                        "2029" => out.push_str("\\P"),
                        other if other.starts_with("00") => {
                            out.push_str("\\x");
                            out.push_str(&other[2..]);
                        }
                        _ => {
                            out.extend(chars[i as usize..(i as usize + 6).min(chars.len())].iter())
                        }
                    }
                    i += 5;
                    start = i as usize + 1;
                }
                Some('n') => {
                    let short =
                        at(i + 2) == Some('"') || chars.len() < DOUBLE_QUOTED_MIN_MULTI_LINE;
                    if short {
                        i += 1;
                    } else {
                        // 折行会吃掉第一个换行，所以写两个换行再加缩进。
                        out.push_str(&chars[start..i as usize].iter().collect::<String>());
                        out.push_str("\n\n");
                        while at(i + 2) == Some('\\')
                            && at(i + 3) == Some('n')
                            && at(i + 4) != Some('"')
                        {
                            out.push('\n');
                            i += 2;
                        }
                        out.push_str(&" ".repeat(indent));
                        if at(i + 2) == Some(' ') {
                            out.push('\\');
                        }
                        i += 1;
                        start = i as usize + 1;
                    }
                }
                _ => i += 1,
            }
        }
        i += 1;
    }
    if start > 0 {
        out.extend(chars[start.min(chars.len())..].iter());
    }
    if start == 0 { json.to_owned() } else { out }
}

/// 块标量：尾部换行决定 chomping（无 → `-`，一个 → 空，多个 → `+`）。
/// TS `blockString`：有逻辑行超过 `80 - indent` 时改用折叠块（`>`），折不开（单词本身超宽）时退回字面量（`|`）。
fn block_literal(value: &str, indent: usize) -> String {
    let trailing = value.len() - value.trim_end_matches('\n').len();
    let chomp = match trailing {
        0 => "-",
        1 => "",
        _ => "+",
    };
    let body = value.trim_end_matches('\n');
    let pad = " ".repeat(indent);
    let limit = LINE_WIDTH.saturating_sub(indent);
    let over = body.split('\n').any(|line| line.chars().count() > limit);
    // 以空白开头的行（more-indented）折叠规则另有处理，这里保守地用字面量。
    let more_indented = body.split('\n').any(|line| line.starts_with([' ', '\t']));
    if over
        && !more_indented
        && let Some(folded) = fold_block(body, indent)
    {
        // TS `'>' + header + '\n' + indent + body`：首行前一定换行并缩进。
        let mut out = format!(">{chomp}\n{pad}{folded}");
        for _ in 1..trailing {
            out.push('\n');
        }
        return out;
    }
    let header = format!("|{chomp}");
    let mut out = header;
    for line in body.split('\n') {
        out.push('\n');
        if !line.is_empty() {
            out.push_str(&pad);
            out.push_str(line);
        }
    }
    for _ in 1..trailing {
        out.push('\n');
    }
    out
}

/// JS `Number#toString`。
pub fn js_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(0.0);
    js_float(f)
}

pub fn js_float(f: f64) -> String {
    if f == 0.0 {
        return "0".into();
    }
    if !f.is_finite() {
        return if f.is_nan() {
            "NaN".into()
        } else if f > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let sign = if f < 0.0 { "-" } else { "" };
    // 最短往返的有效数字与指数（Rust `{:e}` 与 JS 同为最短表示）。
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap();
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().unwrap() + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let exp = if e >= 0 {
            format!("+{e}")
        } else {
            e.to_string()
        };
        if k == 1 {
            format!("{digits}e{exp}")
        } else {
            format!("{}.{}e{exp}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}
