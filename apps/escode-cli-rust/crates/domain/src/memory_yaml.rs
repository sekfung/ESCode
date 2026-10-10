//! 记忆文件 frontmatter 的 YAML 子集（TS 用 `yaml` 包完整解析，只取 description 与 type）。
//! 支持：顶层 `key: value`、单/双引号与 plain 标量（含行尾注释）、`|`/`>` 块标量、一层嵌套映射与
//! 简单 flow 映射；非字符串标量（core schema 的 null/bool/数字）视为缺失；重复键或 plain 标量中的
//! `: ` 按 YAML 报错处理，整段视为无 frontmatter（与 TS parse 失败回退一致）。
use std::collections::BTreeMap;

const TYPES: [&str; 4] = ["user", "feedback", "project", "reference"];

#[derive(Debug, Clone)]
enum Node {
    Str(String),
    Other,
    Map(BTreeMap<String, Node>),
}

pub fn description_and_type(preview: &str) -> (Option<String>, Option<String>) {
    let normalized = preview
        .strip_prefix('\u{feff}')
        .unwrap_or(preview)
        .replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if lines.first() != Some(&"---") {
        return (None, None);
    }
    let Some(end) = lines
        .iter()
        .skip(1)
        .position(|l| *l == "---")
        .map(|i| i + 1)
    else {
        return (None, None);
    };
    let Some(map) = parse_mapping(&lines[1..end], 0) else {
        return (None, None);
    };
    let description = match map.get("description") {
        Some(Node::Str(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    let candidate = match map.get("metadata") {
        Some(Node::Map(m)) => m.get("type").or_else(|| map.get("type")),
        _ => map.get("type"),
    };
    let kind = match candidate {
        Some(Node::Str(s)) if TYPES.contains(&s.as_str()) => Some(s.clone()),
        _ => None,
    };
    (description, kind)
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}
fn is_ignorable(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

fn parse_mapping(lines: &[&str], indent: usize) -> Option<BTreeMap<String, Node>> {
    let mut map = BTreeMap::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if is_ignorable(line) {
            continue;
        }
        if indent_of(line) != indent {
            return None;
        }
        let content = &line[indent..];
        let (key, value) = split_key(content)?;
        let value = value.trim();
        // 收集属于该键的缩进子块。
        let start = i;
        while i < lines.len() && (is_ignorable(lines[i]) || indent_of(lines[i]) > indent) {
            i += 1;
        }
        let child = &lines[start..i];
        let node = if value.is_empty() {
            let nested: Vec<&str> = child.to_vec();
            match nested.iter().find(|l| !is_ignorable(l)) {
                None => Node::Other,
                Some(first) if first.trim_start().starts_with("- ") => Node::Other,
                Some(first) => Node::Map(parse_mapping(&nested, indent_of(first))?),
            }
        } else if let Some(style) = block_style(value) {
            Node::Str(block_scalar(style, child))
        } else if value.starts_with('{') {
            Node::Map(parse_flow_mapping(value)?)
        } else if value.starts_with('[') {
            Node::Other
        } else {
            let mut text = value.to_owned();
            // plain 标量可跨行折叠（续行缩进更深）。
            if !value.starts_with(['"', '\'']) {
                for extra in child.iter().filter(|l| !l.trim().is_empty()) {
                    text.push(' ');
                    text.push_str(extra.trim());
                }
            }
            scalar(&text)?
        };
        if map.insert(key, node).is_some() {
            return None;
        }
    }
    Some(map)
}
fn split_key(content: &str) -> Option<(String, &str)> {
    if let Some(rest) = content.strip_prefix('"') {
        let end = rest.find('"')?;
        let after = rest[end + 1..].strip_prefix(':')?;
        return Some((rest[..end].to_owned(), after));
    }
    let position = content
        .char_indices()
        .find(|(i, c)| *c == ':' && content[i + 1..].chars().next().is_none_or(|n| n == ' '))
        .map(|(i, _)| i)?;
    Some((
        content[..position].trim().to_owned(),
        &content[position + 1..],
    ))
}
fn block_style(value: &str) -> Option<(char, char)> {
    let value = value.split(" #").next().unwrap_or(value).trim();
    let mut chars = value.chars();
    let kind = chars.next().filter(|c| *c == '|' || *c == '>')?;
    let chomp = match chars.as_str() {
        "" => 'c',
        "-" => '-',
        "+" => '+',
        _ => return None,
    };
    Some((kind, chomp))
}
fn block_scalar((kind, chomp): (char, char), child: &[&str]) -> String {
    let indent = child
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .unwrap_or(0);
    let body: Vec<String> = child
        .iter()
        .map(|l| l.get(indent..).unwrap_or("").to_owned())
        .collect();
    let mut text = if kind == '|' {
        body.join("\n")
    } else {
        let mut folded = String::new();
        for (index, line) in body.iter().enumerate() {
            if index > 0 {
                folded.push(if line.is_empty() || body[index - 1].is_empty() {
                    '\n'
                } else {
                    ' '
                });
            }
            folded.push_str(line);
        }
        folded
    };
    let content = text.trim_end_matches('\n').to_owned();
    match chomp {
        '-' => content,
        '+' => {
            text.push('\n');
            text
        }
        _ if content.is_empty() => content,
        _ => content + "\n",
    }
}
fn parse_flow_mapping(value: &str) -> Option<BTreeMap<String, Node>> {
    let inner = value.strip_prefix('{')?.trim_end();
    let inner = inner.strip_suffix('}')?;
    let mut map = BTreeMap::new();
    for pair in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once(':')?;
        if map
            .insert(key.trim().to_owned(), scalar(value.trim())?)
            .is_some()
        {
            return None;
        }
    }
    Some(map)
}
/// 单行标量：引号串按 YAML 转义，plain 串去掉行尾注释后按 core schema 判定类型。
fn scalar(value: &str) -> Option<Node> {
    if let Some(rest) = value.strip_prefix('\'') {
        let end = rest.rfind('\'')?;
        return Some(Node::Str(rest[..end].replace("''", "'")));
    }
    if let Some(rest) = value.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(Node::Str(out)),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                },
                other => out.push(other),
            }
        }
        return None;
    }
    let plain = value.split(" #").next().unwrap_or(value).trim();
    if plain.contains(": ") {
        return None;
    }
    Some(if non_string(plain) {
        Node::Other
    } else {
        Node::Str(plain.to_owned())
    })
}
fn non_string(plain: &str) -> bool {
    // YAML 1.2 core schema：null / bool / int（十、八、十六进制）/ float（含 .inf/.nan）。
    static CORE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"^(~|null|Null|NULL|true|True|TRUE|false|False|FALSE",
            r"|[-+]?[0-9]+|0o[0-7]+|0x[0-9a-fA-F]+",
            r"|[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?",
            r"|[-+]?\.(inf|Inf|INF)|\.nan|\.NaN|\.NAN)$"
        ))
        .unwrap()
    });
    plain.is_empty() || CORE.is_match(plain)
}

/// frontmatter 能否按本子集解析（TS `parseDocument` 无错误）。
pub fn is_valid(frontmatter: &str) -> bool {
    let normalized = frontmatter.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    parse_mapping(&lines, 0).is_some()
}
