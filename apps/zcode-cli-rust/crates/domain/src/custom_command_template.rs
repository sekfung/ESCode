//! 自定义命令的模板展开、提示词格式与 shell 展开切分（TS `contracts/src/commands/index.ts`、
//! `bootstrap/src/custom-command-shell-expansion.ts`）。实际执行由 tools 负责。
use super::custom_command::{js_trim, js_ws};
use regex::Regex;
use std::{path::Path, sync::LazyLock};

pub const SHELL_TIMEOUT_MS: u64 = 30_000;
pub const SHELL_OUTPUT_BYTES: usize = 128 * 1024;
const ALL_ARGUMENTS: &str = "$ARGUMENTS";

static POSITIONAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$(\d+)").unwrap());
static INLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!`([^`]*)`").unwrap());
static FENCED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```!\s*\r?\n?([\s\S]*?)```").unwrap());
static CONTEXT_VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{(CLAUDE_CODE_SESSION_ID|CLAUDE_PLUGIN_DATA|CLAUDE_PLUGIN_ROOT|CLAUDE_PROJECT_DIR|CLAUDE_SESSION_ID|CLAUDE_SKILL_DIR|ZCODE_PLUGIN_DATA|ZCODE_PLUGIN_ROOT|ZCODE_PROJECT_DIR|ZCODE_SESSION_ID|ZCODE_SKILL_DIR)\}").unwrap()
});

/// 输入是否为 `/name args` 形式（TS parsePromptCustomCommandInvocation）。
pub fn parse_invocation(input: &str) -> Option<(String, String)> {
    let trimmed = js_trim(input);
    let rest = trimmed.strip_prefix('/')?;
    let end = rest.find(js_ws).unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() {
        return None;
    }
    Some((name.to_lowercase(), js_trim(&rest[end..]).to_owned()))
}

pub fn split_arguments(input: &str) -> Vec<String> {
    let mut args = vec![];
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaping = false;
    for c in input.chars() {
        if escaping {
            current.push(c);
            escaping = false;
        } else if c == '\\' {
            escaping = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if js_ws(c) {
            if !current.is_empty() {
                args.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if escaping {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub struct Expansion {
    pub body: String,
    pub argument_count: usize,
}
pub fn expand_template(content: &str, args: &str) -> Expansion {
    let args = js_trim(args);
    let positional = split_arguments(args);
    let mut used = content.contains(ALL_ARGUMENTS);
    let replaced = content.replace(ALL_ARGUMENTS, args);
    let mut body = POSITIONAL
        .replace_all(&replaced, |c: &regex::Captures| {
            used = true;
            // JS Number("007") - 1；超出 usize 的序号同样取不到参数。
            c[1].parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| positional.get(i))
                .cloned()
                .unwrap_or_default()
        })
        .into_owned();
    if !args.is_empty() && !used {
        body = format!(
            "{}\n\nUser arguments:\n{args}",
            body.trim_end_matches(js_ws)
        );
    }
    Expansion {
        body,
        argument_count: positional.len(),
    }
}
pub fn format_prompt(
    name: &str,
    scope: &str,
    source: &str,
    skills: &[String],
    body: &str,
) -> String {
    let mut lines = vec![
        format!("Run custom command /{name}."),
        format!("Command source: {scope}/{source}."),
    ];
    if !skills.is_empty() {
        let names = skills
            .iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Required skills: {names}."));
        lines.push(format!(
            "Before following the command body, call the Skill tool for {names}."
        ));
    }
    lines.push(String::new());
    lines.push(js_trim(body).to_owned());
    lines.join("\n")
}

#[derive(Debug, PartialEq)]
pub enum Segment {
    Text(String),
    Shell(String),
}
/// 按出现顺序切出 `!`cmd`` 与 ```! 块；同位置时 fenced 优先（TS pickNextMatch）。
pub fn shell_segments(content: &str) -> Vec<Segment> {
    let mut segments = vec![];
    let mut cursor = 0;
    while cursor < content.len() {
        let inline = INLINE.captures_at(content, cursor);
        let fenced = FENCED.captures_at(content, cursor);
        let start = |c: &Option<regex::Captures>| c.as_ref().map(|c| c.get(0).unwrap().start());
        let (captures, fenced_match) = match (start(&inline), start(&fenced)) {
            (None, None) => break,
            (Some(i), Some(f)) if f <= i => (fenced.unwrap(), true),
            (None, Some(_)) => (fenced.unwrap(), true),
            _ => (inline.unwrap(), false),
        };
        let whole = captures.get(0).unwrap();
        segments.push(Segment::Text(content[cursor..whole.start()].into()));
        let raw = &captures[1];
        let command = if fenced_match {
            let raw = raw
                .strip_prefix("\r\n")
                .or_else(|| raw.strip_prefix('\n'))
                .unwrap_or(raw);
            let raw = raw
                .strip_suffix("\r\n")
                .or_else(|| raw.strip_suffix('\n'))
                .unwrap_or(raw);
            js_trim(raw)
        } else {
            js_trim(raw)
        };
        segments.push(Segment::Shell(command.into()));
        cursor = whole.end();
    }
    segments.push(Segment::Text(content[cursor.min(content.len())..].into()));
    segments
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginContext {
    pub data_path: String,
    pub id: String,
    pub name: String,
    pub root_path: String,
}
/// 插件命令缺少显式上下文时由根目录推断（TS inferPluginContext）。
pub fn infer_plugin(source: &str, root_path: &str) -> Option<PluginContext> {
    let root = Path::new(root_path);
    if source != "plugin" || root.file_name()? != "commands" {
        return None;
    }
    let plugin_root = root.parent()?;
    let name = plugin_root.file_name()?.to_string_lossy().into_owned();
    let path = plugin_root.to_string_lossy().into_owned();
    Some(PluginContext {
        data_path: path.clone(),
        id: name.clone(),
        name,
        root_path: path,
    })
}
/// 变量需要的上下文缺失时报错（TS assertShellExpansionContextAvailable）。
pub fn check_shell_context(
    name: &str,
    command: &str,
    has_session: bool,
    has_plugin: bool,
) -> Result<(), String> {
    for captures in CONTEXT_VARIABLE.captures_iter(command) {
        let variable = &captures[1];
        let requirement = match variable {
            "CLAUDE_SKILL_DIR" | "ZCODE_SKILL_DIR" => Some("a skill context"),
            "CLAUDE_CODE_SESSION_ID" | "CLAUDE_SESSION_ID" | "ZCODE_SESSION_ID" if !has_session => {
                Some("a runtime session context")
            }
            "CLAUDE_PLUGIN_DATA" | "CLAUDE_PLUGIN_ROOT" | "ZCODE_PLUGIN_DATA"
            | "ZCODE_PLUGIN_ROOT"
                if !has_plugin =>
            {
                Some("a plugin context")
            }
            _ => None,
        };
        if let Some(requirement) = requirement {
            return Err(format!(
                "Custom command /{name} variable requires {requirement}: {variable}"
            ));
        }
    }
    Ok(())
}
pub fn shell_env(
    cwd: &str,
    session: Option<&str>,
    plugin: Option<&PluginContext>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("CLAUDE_PROJECT_DIR".into(), cwd.into()),
        ("ZCODE_PROJECT_DIR".into(), cwd.into()),
    ];
    if let Some(session) = session {
        for key in [
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_SESSION_ID",
            "ZCODE_SESSION_ID",
        ] {
            env.push((key.into(), session.into()));
        }
    }
    if let Some(p) = plugin {
        env.extend([
            ("CLAUDE_PLUGIN_DATA".into(), p.data_path.clone()),
            ("CLAUDE_PLUGIN_ROOT".into(), p.root_path.clone()),
            ("ZCODE_PLUGIN_DATA".into(), p.data_path.clone()),
            ("ZCODE_PLUGIN_ID".into(), p.id.clone()),
            ("ZCODE_PLUGIN_NAME".into(), p.name.clone()),
            ("ZCODE_PLUGIN_ROOT".into(), p.root_path.clone()),
        ]);
    }
    env
}
/// 失败文案（TS formatShellExpansionError）；`exit` 为退出码或状态名。
pub fn shell_failure(
    name: &str,
    command: &str,
    exit: &str,
    error: Option<&str>,
    stderr: &str,
    stdout: &str,
    status: &str,
) -> String {
    let preview = |s: &str| super::custom_command::truncate_utf16(js_trim(s), 2_000);
    let details = error
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .or_else(|| Some(preview(stderr)).filter(|s| !s.is_empty()))
        .or_else(|| Some(preview(stdout)).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| format!("status={status}"));
    format!(
        "Custom command /{name} shell expansion failed.\nCommand: {command}\nExit: {exit}\nDetails: {details}"
    )
}

/// 插件 manifest `commands` 对象映射生成的命令 markdown（TS applyCommandMetadataFrontmatter）。
pub fn generated_command_markdown(markdown: &str, metadata: &serde_json::Value) -> String {
    let mut frontmatter = vec![];
    let text = |key: &str| {
        metadata[key]
            .as_str()
            .map(js_trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    if let Some(v) = text("description") {
        frontmatter.push(("description", v));
    }
    if let Some(v) = text("argumentHint") {
        frontmatter.push(("argument-hint", v));
    }
    if let Some(v) = text("model") {
        frontmatter.push(("model", v));
    }
    if let Some(tools) = metadata["allowedTools"].as_array() {
        let tools = tools
            .iter()
            .filter_map(|t| t.as_str().map(js_trim).filter(|t| !t.is_empty()))
            .collect::<Vec<_>>();
        if !tools.is_empty() {
            frontmatter.push(("allowed-tools", tools.join(", ")));
        }
    }
    if frontmatter.is_empty() {
        return markdown.into();
    }
    let normalized = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let lines: Vec<&str> = normalized
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let body = if normalized.starts_with("---") && js_trim(lines[0]) == "---" {
        match (1..lines.len()).find(|i| js_trim(lines[*i]) == "---") {
            Some(end) => lines[end + 1..].join("\n"),
            None => markdown.into(),
        }
    } else {
        markdown.into()
    };
    let header = frontmatter
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("---\n{header}\n---\n\n{}", body.trim_start_matches(js_ws))
}
/// 生成命令名（TS normalizeGeneratedCommandName）。
pub fn generated_command_name(name: &str) -> Option<String> {
    let name = super::custom_command::normalize_name(name);
    super::custom_command::valid_name(&name).then_some(name)
}
/// 插件数据目录名（TS sanitizePluginId）。
pub fn sanitize_plugin_id(id: &str) -> String {
    // JS 正则按 UTF-16 码元替换：BMP 外字符替换成两个 `-`。
    id.chars()
        .flat_map(|c| {
            let keep = c.is_ascii_alphanumeric() || "_.@-".contains(c);
            let count = if keep { 1 } else { c.len_utf16() };
            std::iter::repeat_n(if keep { c } else { '-' }, count)
        })
        .collect()
}
