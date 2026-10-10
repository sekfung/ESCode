//! 自定义 slash 命令的纯规则（docs/specs/rust-custom-commands.md）：命名、frontmatter、协议目录与保留名。
//! 模板展开与 shell 切分见 custom_command_template.rs。逐条对齐 TS `adapters/src/commands/index.ts`。
use serde_json::{Value, json};
use std::{cmp::Ordering, collections::BTreeMap, sync::LazyLock};

pub use super::custom_command_template::*;

pub const MAX_COMMAND_BYTES: usize = 100_000;
pub const MAX_SCAN_DEPTH: usize = 12;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

static ASSET: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("slash_commands.json")).unwrap());

/// App 协议目录中的内置段（TS listAppProtocolBuiltinSlashCommands，动态工作流关闭）。
pub fn builtin_catalog() -> Vec<Value> {
    ASSET["builtins"].as_array().cloned().unwrap_or_default()
}
pub fn is_reserved(name: &str) -> bool {
    let name = normalize_name(name);
    ASSET["reserved"]
        .as_array()
        .is_some_and(|r| r.iter().any(|v| v == name.as_str()))
}
pub fn normalize_name(name: &str) -> String {
    js_trim(name).trim_start_matches('/').to_lowercase()
}
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_:-".contains(b))
}
/// TS `localeCompare`（ICU 根排序）在合法命令名字符集上的等价序：标点先于数字先于字母。
pub fn compare_names(a: &str, b: &str) -> Ordering {
    let rank = |c: char| match c {
        '_' => 0,
        '-' => 1,
        ':' => 2,
        '0'..='9' => 3 + c as u32 - '0' as u32,
        'a'..='z' => 13 + c as u32 - 'a' as u32,
        other => 100 + other as u32,
    };
    a.chars().map(rank).cmp(b.chars().map(rank))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub allowed_tools: Vec<String>,
    pub argument_hint: Option<String>,
    pub description: String,
    pub disable_non_interactive: bool,
    pub frontmatter_keys: Vec<String>,
    pub model: Option<String>,
    pub name: String,
    pub scope: String,
    pub skills: Vec<String>,
    pub source: String,
}
impl Metadata {
    /// 协议 `slashCommands` 条目（TS listProtocolSlashCommands 的 custom 段）。
    pub fn catalog_entry(&self) -> Value {
        let hint = self
            .argument_hint
            .as_ref()
            .map(|h| format!(" {h}"))
            .unwrap_or_default();
        json!({"description":self.description,"inputHint":format!("/{}{hint}",self.name),"name":self.name,"source":"custom"})
    }
}

/// JS `\s` / `String.prototype.trim` 的空白集合（多出 BOM）。
pub fn js_ws(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(js_ws)
}
fn without_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}
fn split_lines(content: &str) -> Vec<&str> {
    content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}
fn frontmatter_end(lines: &[&str]) -> Option<usize> {
    (1..lines.len()).find(|i| js_trim(lines[*i]) == "---")
}
fn extract_frontmatter(content: &str) -> Option<String> {
    let normalized = without_bom(content);
    if !normalized.starts_with("---") {
        return None;
    }
    let lines = split_lines(normalized);
    if js_trim(lines[0]) != "---" {
        return None;
    }
    frontmatter_end(&lines).map(|end| lines[1..end].join("\n"))
}
pub fn strip_frontmatter(content: &str) -> String {
    let normalized = without_bom(content);
    if !normalized.starts_with("---") {
        return content.into();
    }
    let lines = split_lines(normalized);
    match frontmatter_end(&lines) {
        Some(end) => lines[end + 1..].join("\n"),
        None => content.into(),
    }
}
fn parse_flat_yaml(frontmatter: &str) -> (Vec<String>, BTreeMap<String, String>) {
    let mut keys = vec![];
    let mut values = BTreeMap::new();
    for line in frontmatter.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if js_trim(line).is_empty() || js_trim(line).starts_with('#') || line.starts_with(js_ws) {
            continue;
        }
        let Some(separator) = line.find(':').filter(|i| *i > 0) else {
            continue;
        };
        let key = js_trim(&line[..separator]).to_owned();
        keys.push(key.clone());
        values.insert(key, js_trim(&line[separator + 1..]).to_owned());
    }
    (keys, values)
}
fn parse_scalar(value: Option<&String>) -> Option<String> {
    let trimmed = js_trim(value?);
    if trimmed.is_empty() {
        return None;
    }
    let quoted = (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''));
    Some(match quoted {
        // 单个引号字符时 TS slice(1,-1) 得到空串。
        true if trimmed.len() == 1 => String::new(),
        true => js_trim(&trimmed[1..trimmed.len() - 1]).to_owned(),
        false => trimmed.to_owned(),
    })
}
fn parse_list(value: Option<&String>) -> Vec<String> {
    let Some(scalar) = parse_scalar(value).filter(|s| !s.is_empty()) else {
        return vec![];
    };
    let scalar = scalar.strip_prefix('[').unwrap_or(&scalar);
    let scalar = scalar.strip_suffix(']').unwrap_or(scalar);
    scalar
        .split(',')
        .map(js_trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
/// JS `slice(0, n)` 按 UTF-16 码元截断。
pub fn truncate_utf16(value: &str, max: usize) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    if units.len() <= max {
        value.into()
    } else {
        String::from_utf16_lossy(&units[..max])
    }
}
/// TS `.replace(/^#+\s*/, "").replace(/^[-*]\s*/, "").trim()` 后取首个非空行。
fn extract_description(body: &str) -> Option<String> {
    split_lines(body)
        .into_iter()
        .map(|line| {
            let mut s = line;
            if s.starts_with('#') {
                s = s.trim_start_matches('#').trim_start_matches(js_ws);
            }
            if s.starts_with(['-', '*']) {
                s = s[1..].trim_start_matches(js_ws);
            }
            js_trim(s).to_owned()
        })
        .find(|c| !c.is_empty())
        .map(|line| truncate_utf16(&line, MAX_DESCRIPTION_LENGTH))
}

/// 解析单个命令文件；None 表示 TS 会跳过该文件（名称非法或缺少描述）。
pub fn parse_command(raw: &str, name: &str, scope: &str, source: &str) -> Option<Metadata> {
    if !valid_name(name) {
        return None;
    }
    let (keys, values) = extract_frontmatter(raw)
        .map(|f| parse_flat_yaml(&f))
        .unwrap_or_default();
    let body = command_content(raw);
    let description = parse_scalar(values.get("description"))
        .or_else(|| extract_description(&body))
        .filter(|d| !d.is_empty())?;
    Some(Metadata {
        allowed_tools: parse_list(values.get("allowed-tools")),
        argument_hint: parse_scalar(values.get("argument-hint")),
        description: truncate_utf16(&description, MAX_DESCRIPTION_LENGTH),
        disable_non_interactive: parse_scalar(values.get("disable-noninteractive"))
            .is_some_and(|v| matches!(v.to_lowercase().as_str(), "true" | "yes")),
        frontmatter_keys: keys,
        model: parse_scalar(values.get("model")),
        name: name.into(),
        scope: scope.into(),
        skills: parse_list(values.get("skills")),
        source: source.into(),
    })
}
/// 命令正文：去掉 frontmatter 后 trim（TS loadCommand）。
pub fn command_content(raw: &str) -> String {
    js_trim(&strip_frontmatter(raw)).to_owned()
}
