//! `SKILL.md` frontmatter 解析（TS `skillsService.ts` 的 `readFrontmatter` 及宽松回退）。
//!
//! 严格路径用 saphyr（YAML 1.2 core schema，与 TS `yaml` 同族）；解析失败、非映射、含未加引号的 `: ` 等
//! 历史写法时回退到逐行宽松读取，`parseOk=false`。

use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};

use crate::git::paths::{is_js_space, js_trim};

#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    pub has_frontmatter: bool,
    pub name: String,
    pub description: String,
    pub body: String,
    pub keys: Vec<String>,
    pub parse_ok: bool,
}

/// `/^---\n([\s\S]*?)\n---(?:\n|$)/`：返回（frontmatter 文本，整段匹配长度）。
fn split_frontmatter(normalized: &str) -> Option<(&str, usize)> {
    let rest = normalized.strip_prefix("---\n")?;
    let mut from = 0;
    while let Some(found) = rest[from..].find("\n---") {
        let at = from + found;
        let after = at + 4;
        match rest.as_bytes().get(after) {
            None => return Some((&rest[..at], 4 + after)),
            Some(b'\n') => return Some((&rest[..at], 4 + after + 1)),
            _ => from = at + 1,
        }
    }
    None
}

/// `/^#{1,6}\s+\S/`。
fn is_heading_line(line: &str) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let rest = &line[hashes..];
    let after_space = rest.trim_start_matches(is_js_space);
    after_space.len() < rest.len() && !after_space.is_empty()
}

fn split_leaked_body(text: &str) -> (String, String) {
    let lines: Vec<&str> = text.split('\n').collect();
    match lines
        .iter()
        .enumerate()
        .position(|(i, line)| i > 0 && is_heading_line(line))
    {
        None => (text.to_owned(), String::new()),
        Some(index) => (
            lines[..index]
                .join("\n")
                .trim_end_matches(is_js_space)
                .to_owned(),
            js_trim(&lines[index..].join("\n")).to_owned(),
        ),
    }
}

/// `readLooseFrontmatterInlineString`：`trim().replace(/^["']|["']$/g, "")`（首尾各去一个引号）。
fn loose_inline(value: &str) -> String {
    let trimmed = js_trim(value);
    let without_head = trimmed.strip_prefix(['"', '\'']).unwrap_or(trimmed);
    without_head
        .strip_suffix(['"', '\''])
        .unwrap_or(without_head)
        .to_owned()
}

fn loose_fields(text: &str) -> (String, String) {
    let (mut name, mut description) = (String::new(), String::new());
    let lines: Vec<&str> = text.split('\n').collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(colon) = line.find(':') {
            let key = js_trim(&line[..colon]);
            let raw = js_trim(&line[colon + 1..]);
            if key == "name" {
                name = loose_inline(raw);
            } else if key == "description" {
                let style = raw.chars().next();
                if style == Some('|') || style == Some('>') {
                    let mut block: Vec<String> = Vec::new();
                    let mut next = index + 1;
                    while next < lines.len() {
                        let candidate = lines[next];
                        if !candidate.starts_with([' ', '\t']) {
                            break;
                        }
                        block.push(strip_up_to_two_spaces(candidate));
                        index = next;
                        next += 1;
                    }
                    description = if style == Some('>') {
                        block.join(" ")
                    } else {
                        block.join("\n")
                    };
                } else {
                    description = loose_inline(raw);
                }
            }
        }
        index += 1;
    }
    (name, description)
}

/// `replace(/^\s{1,2}/, "")`：去掉至多两个开头空白字符。
fn strip_up_to_two_spaces(line: &str) -> String {
    let mut chars = line.char_indices();
    let mut cut = 0;
    for _ in 0..2 {
        match chars.next() {
            Some((index, ch)) if is_js_space(ch) => cut = index + ch.len_utf8(),
            _ => break,
        }
    }
    line[cut..].to_owned()
}

fn loose_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for line in text.split('\n') {
        if line.is_empty()
            || line.chars().next().is_some_and(is_js_space)
            || js_trim(line).starts_with('#')
        {
            continue;
        }
        let Some(colon) = line.find(':').filter(|c| *c > 0) else {
            continue;
        };
        let key = js_trim(&line[..colon]);
        if !key.is_empty() && !keys.iter().any(|k| k == key) {
            keys.push(key.to_owned());
        }
    }
    keys
}

/// saphyr 的映射本身去重（后者覆盖），而 TS `yaml` 对重复键报错：用顶层键行补上这条判定。
fn has_duplicate_top_level_key(text: &str) -> bool {
    let mut seen: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.is_empty()
            || line.chars().next().is_some_and(is_js_space)
            || js_trim(line).starts_with('#')
        {
            continue;
        }
        let Some(colon) = line.find(':').filter(|c| *c > 0) else {
            continue;
        };
        let key = js_trim(&line[..colon]);
        if seen.contains(&key) {
            return true;
        }
        seen.push(key);
    }
    false
}

fn has_unsafe_inline_field(text: &str) -> bool {
    text.split('\n').any(|line| {
        let Some(colon) = line.find(':') else {
            return false;
        };
        let key = js_trim(&line[..colon]);
        if key != "name" && key != "description" {
            return false;
        }
        let raw = js_trim(&line[colon + 1..]);
        if raw.is_empty() || raw.starts_with(['"', '\'', '|', '>']) {
            return false;
        }
        raw.contains(": ")
    })
}

/// JS `String(value)`（仅覆盖 YAML 标量/序列/映射会产生的值）。
fn js_string(node: &YamlOwned) -> String {
    match node {
        YamlOwned::Value(ScalarOwned::Null) => String::new(),
        YamlOwned::Value(ScalarOwned::Boolean(flag)) => flag.to_string(),
        YamlOwned::Value(ScalarOwned::Integer(number)) => number.to_string(),
        YamlOwned::Value(ScalarOwned::FloatingPoint(number)) => js_number(number.0),
        YamlOwned::Value(ScalarOwned::String(text)) => text.clone(),
        YamlOwned::Representation(text, ..) => text.clone(),
        YamlOwned::Sequence(items) => items
            .iter()
            .map(|item| {
                if matches!(item, YamlOwned::Value(ScalarOwned::Null)) {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        YamlOwned::Mapping(_) => "[object Object]".to_owned(),
        YamlOwned::Tagged(_, inner) => js_string(inner),
        _ => String::new(),
    }
}

fn js_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn mapping_key(node: &YamlOwned) -> String {
    match node {
        YamlOwned::Value(ScalarOwned::String(text)) => text.clone(),
        other => js_string(other),
    }
}

/// 严格解析：`Some((name, description, keys))` 仅当结果是无重复键的映射。
fn strict(text: &str) -> Option<(String, String, Vec<String>)> {
    let documents = YamlOwned::load_from_str(text).ok()?;
    if documents.len() != 1 || has_duplicate_top_level_key(text) {
        return None;
    }
    let YamlOwned::Mapping(map) = &documents[0] else {
        return None;
    };
    let mut keys: Vec<String> = Vec::new();
    let (mut name, mut description) = (None, None);
    for (key, value) in map {
        let key = mapping_key(key);
        if keys.contains(&key) {
            return None;
        }
        if key == "name" {
            name = Some(js_string(value));
        } else if key == "description" {
            description = Some(js_string(value));
        }
        keys.push(key);
    }
    Some((
        name.unwrap_or_default(),
        description.unwrap_or_default(),
        keys,
    ))
}

pub fn read(content: &str) -> Parsed {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let Some((frontmatter, consumed)) = split_frontmatter(&normalized) else {
        return Parsed {
            has_frontmatter: false,
            name: String::new(),
            description: String::new(),
            body: js_trim(&normalized).to_owned(),
            keys: Vec::new(),
            parse_ok: false,
        };
    };
    let (metadata, leaked) = split_leaked_body(frontmatter);
    let after = js_trim(&normalized[consumed..]).to_owned();
    let body = js_trim(
        &[leaked, after]
            .into_iter()
            .filter(|part| !js_trim(part).is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
    .to_owned();
    let (loose_name, loose_description) = loose_fields(&metadata);
    let fallback = |keys: Vec<String>| Parsed {
        has_frontmatter: true,
        name: loose_name.clone(),
        description: loose_description.clone(),
        body: body.clone(),
        keys,
        parse_ok: false,
    };
    if has_unsafe_inline_field(&metadata) {
        return fallback(loose_keys(&metadata));
    }
    match strict(&metadata) {
        None => fallback(loose_keys(&metadata)),
        Some((name, description, keys)) => Parsed {
            has_frontmatter: true,
            name: if name.is_empty() { loose_name } else { name },
            description: if description.is_empty() {
                loose_description
            } else {
                description
            },
            body,
            keys,
            parse_ok: true,
        },
    }
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;
