//! 命令文件的 markdown frontmatter 解析与生成（TS `commandFileParser.ts`，165 行）。
//!
//! 逐条复刻：`---` 片段取**首个**行首分隔与紧随其后的第二个；键名 `[A-Za-z0-9_-]+` 且小写归一；
//! 多行值只收**缩进行**并以空格拼接；生成时保留未替换的键（`description` / `argument-hint` 两个除外）。

use serde_json::{Map, Value};

pub struct ParsedCommandFile {
    pub name: String,
    pub prompt: String,
    pub content: String,
    pub file_path: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// TS `getCrossPlatformBasename`：按 `\` 或 `/` 取最后一段。
fn basename(file_path: &str) -> &str {
    file_path
        .split(['\\', '/'])
        .rfind(|part| !part.is_empty())
        .unwrap_or("")
}

/// TS 的 `line.trim() === "---"`：JS 的 `\s` 口径（含 `\uFEFF`、不含 `\u0085`）。
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

/// TS `splitMarkdownCommandContent`。
fn split_markdown_command_content(content: &str) -> (Vec<&str>, Vec<&str>) {
    let lines: Vec<&str> = content.split('\n').collect();
    let start = lines.iter().position(|line| js_trim(line) == "---");
    let end = start.and_then(|start| {
        lines
            .iter()
            .enumerate()
            .position(|(index, line)| index > start && js_trim(line) == "---")
    });
    match (start, end) {
        (Some(start), Some(end)) => (lines[start + 1..end].to_vec(), lines[end + 1..].to_vec()),
        _ => (Vec::new(), lines),
    }
}

/// TS `readFrontmatterKey`：缩进行不是键；`^([A-Za-z0-9_-]+)\s*:` 的首组小写。
fn read_frontmatter_key(line: &str) -> Option<String> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let trimmed = js_trim(line);
    let colon = trimmed.find(':')?;
    let head = &trimmed[..colon];
    // `\s*:` 允许键与冒号之间有空白；`head` 去掉尾随空白后必须全是键名字符。
    let name = head.trim_end();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    // 冒号前不允许其他字符：`description x:` 不匹配（`\s*` 只吃空白）。
    if trimmed[..colon].trim_end() != name {
        return None;
    }
    Some(name.to_ascii_lowercase())
}

/// TS `readFrontmatterMultilineValue`。
fn read_multiline_value(lines: &[&str], target: &str) -> Option<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut collecting = false;
    for line in lines {
        if let Some(key) = read_frontmatter_key(line) {
            if collecting {
                break;
            }
            if key == target {
                collecting = true;
                // `line.split(":").slice(1).join(":").trim()`：冒号之后的部分（含后续冒号）。
                let after = line.find(':').map(|index| &line[index + 1..]).unwrap_or("");
                let value = js_trim(after);
                if !value.is_empty() {
                    collected.push(value.to_owned());
                }
            }
            continue;
        }
        if collecting && (line.starts_with(' ') || line.starts_with('\t')) {
            collected.push(js_trim(line).to_owned());
        }
    }
    let value = js_trim(&collected.join(" ")).to_owned();
    (!value.is_empty()).then_some(value)
}

/// TS `preserveFrontmatterLines`：整段跳过被替换键（含它的缩进续行），直到下一个键。
fn preserve_frontmatter_lines(existing: Option<&str>, replaced: &[&str]) -> Vec<String> {
    let (frontmatter, _) = split_markdown_command_content(existing.unwrap_or(""));
    let mut preserved = Vec::new();
    let mut skipping = false;
    for line in frontmatter {
        if let Some(key) = read_frontmatter_key(line) {
            skipping = replaced.contains(&key.as_str());
        }
        if !skipping {
            preserved.push(line.to_owned());
        }
    }
    preserved
}

/// TS `CommandFileParser.parseCommandFile`（markdown 分支；解析失败回 `None`）。
pub fn parse_command_file(content: &str, file_path: &str) -> Option<ParsedCommandFile> {
    let (frontmatter, content_lines) = split_markdown_command_content(content);
    let file_name = basename(file_path);
    // `fileName.replace(/\.md$/i, "")`：只去掉结尾（大小写不敏感）的一个 `.md`。
    let stem =
        if file_name.len() >= 3 && file_name[file_name.len() - 3..].eq_ignore_ascii_case(".md") {
            &file_name[..file_name.len() - 3]
        } else {
            file_name
        };
    let name = format!("/{stem}");
    let body = content_lines.join("\n");
    Some(ParsedCommandFile {
        name,
        prompt: js_trim(&body).to_owned(),
        content: body,
        file_path: file_path.to_owned(),
        description: read_multiline_value(&frontmatter, "description"),
        argument_hint: read_multiline_value(&frontmatter, "argument-hint"),
    })
}

/// TS `generateCommandFileContent` 需要的配置（`CommandConfig` 的写面投影）。
pub struct CommandConfig<'a> {
    pub prompt: &'a str,
    pub description: Option<&'a str>,
    pub argument_hint: Option<&'a str>,
}

/// TS `generateMarkdownCommandFileContent`。
pub fn generate_command_file_content(config: &CommandConfig<'_>, existing: Option<&str>) -> String {
    let mut frontmatter = preserve_frontmatter_lines(existing, &["description", "argument-hint"]);
    if let Some(value) = config.description.map(js_trim).filter(|v| !v.is_empty()) {
        frontmatter.push(format!("description: {value}"));
    }
    if let Some(value) = config.argument_hint.map(js_trim).filter(|v| !v.is_empty()) {
        frontmatter.push(format!("argument-hint: {value}"));
    }
    if frontmatter.is_empty() {
        return config.prompt.to_owned();
    }
    format!("---\n{}\n---\n\n{}", frontmatter.join("\n"), config.prompt)
}

/// 把 `ParsedCommandFile` 投影成 `UserCommand` 的前半部分（`CommandInfo` 字段）。
pub fn parsed_fields(parsed: &ParsedCommandFile) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("name".to_owned(), Value::from(parsed.name.clone()));
    fields.insert("prompt".to_owned(), Value::from(parsed.prompt.clone()));
    fields.insert("content".to_owned(), Value::from(parsed.content.clone()));
    fields.insert("filePath".to_owned(), Value::from(parsed.file_path.clone()));
    if let Some(description) = &parsed.description {
        fields.insert("description".to_owned(), Value::from(description.clone()));
    }
    if let Some(hint) = &parsed.argument_hint {
        fields.insert("argumentHint".to_owned(), Value::from(hint.clone()));
    }
    fields
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
