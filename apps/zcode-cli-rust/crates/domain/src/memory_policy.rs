//! 记忆路径约束、受限 agent 工具策略与提取判定（TS `memory-file-path.ts`、`memory-agent-loop.ts`
//! `evaluateMemoryAgentToolPolicy`、`extraction.ts` `evaluateMemoryExtraction`）。
//! 路径按 Node `path` 的词法语义处理（Windows 两种分隔符、盘符、比较不区分大小写），不访问文件系统。
use serde_json::Value;

const SENSITIVE: [&str; 19] = [
    ".git",
    "hooks",
    ".husky",
    ".githooks",
    "node_modules",
    ".vscode",
    ".idea",
    "head",
    "config",
    "objects",
    "refs",
    ".zcode",
    "skills",
    "commands",
    "agents",
    ".cargo",
    ".devcontainer",
    ".yarn",
    ".mvn",
];

fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}
fn has_drive(path: &str) -> bool {
    cfg!(windows)
        && path.len() >= 2
        && path.as_bytes()[1] == b':'
        && path.as_bytes()[0].is_ascii_alphabetic()
}
/// Node `path.isAbsolute`。
pub fn is_absolute(path: &str) -> bool {
    path.starts_with(is_separator) || (has_drive(path) && path[2..].starts_with(is_separator))
}
/// 词法规范化为（根前缀, 段列表）；Node `path.resolve(base, path)`。
fn resolve(base: &str, path: &str) -> (String, Vec<String>) {
    let full = if is_absolute(path) {
        path.to_owned()
    } else {
        format!("{base}/{path}")
    };
    let prefix = if has_drive(&full) {
        full[..2].to_uppercase()
    } else {
        String::new()
    };
    let mut parts: Vec<String> = vec![];
    for part in full[prefix.len()..].split(is_separator) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_owned()),
        }
    }
    (prefix, parts)
}
fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}
/// 记忆根内的相对段；不在根内（含根本身）时为 None（TS isContainedRelativePath）。
pub fn contained_segments(root: &str, file_path: &str, cwd: &str) -> Option<Vec<String>> {
    let (root_prefix, root_parts) = resolve(cwd, root);
    let (prefix, parts) = resolve(cwd, file_path);
    if !same(&root_prefix, &prefix)
        || parts.len() <= root_parts.len()
        || !root_parts.iter().zip(&parts).all(|(a, b)| same(a, b))
    {
        return None;
    }
    Some(parts[root_parts.len()..].to_vec())
}
fn sensitive(segment: &str) -> bool {
    let cleaned: String = segment
        .to_lowercase()
        .chars()
        .filter(
            |c| !matches!(*c as u32, 0x200c..=0x200f | 0x202a..=0x202e | 0x206a..=0x206f | 0xfeff),
        )
        .collect();
    let head = cleaned.split(':').next().unwrap_or("");
    SENSITIVE.contains(&head.trim_end_matches(['.', ' ']))
}
/// 可放行写入的记忆 Markdown（TS resolveSafeMemoryFilePath + `.md` 后缀）。
pub fn is_safe_memory_markdown(root: &str, file_path: &str, cwd: &str) -> bool {
    file_path.ends_with(".md")
        && contained_segments(root, file_path, cwd)
            .is_some_and(|segments| !segments.iter().any(|s| sensitive(s)))
}

fn deny_tool(root: &str) -> String {
    format!("only Read, Grep, Glob, read-only Bash, and Edit/Write within {root} are allowed")
}
fn deny_bash(root: &str) -> String {
    format!(
        "Only read-only shell commands and rm with all paths inside {root} are permitted in this context (ls, find, grep, cat, stat, wc, head, tail, and similar)"
    )
}
/// 工具目录中的条目：是否存在、是否网络副作用。
pub enum ToolKind {
    Missing,
    Network,
    Local,
}
/// 受限记忆 agent 的工具调用判定；Err 为回给模型的拒绝文案。`readonly_bash` 为带运行时上下文的只读分类。
pub fn tool_policy(
    name: &str,
    input: &Value,
    kind: ToolKind,
    root: &str,
    cwd: &str,
    readonly_bash: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    match kind {
        ToolKind::Missing => {
            return Err(format!(
                "<tool_use_error>Error: No such tool available: {name}</tool_use_error>"
            ));
        }
        ToolKind::Network => return Err(deny_tool(root)),
        ToolKind::Local if name == "Agent" || name.starts_with("mcp__") => {
            return Err(deny_tool(root));
        }
        ToolKind::Local => {}
    }
    match name {
        "Write" | "Edit" => match input["file_path"].as_str() {
            Some(path) if is_safe_memory_markdown(root, path, cwd) => Ok(()),
            _ => Err(deny_tool(root)),
        },
        "Bash" => match input["command"].as_str() {
            Some(command) if readonly_bash(command) || is_memory_rm(command, root, cwd) => Ok(()),
            _ => Err(deny_bash(root)),
        },
        "Read" | "Grep" | "Glob" => Ok(()),
        _ => Err(deny_tool(root)),
    }
}
/// 仅删除记忆根内绝对 `.md` 路径的单条 `rm`（TS isContainedMarkdownBashRemoval）。
fn is_memory_rm(command: &str, root: &str, cwd: &str) -> bool {
    let analysis = super::bash_parse::analyze(command);
    if !analysis.permission_safe() || analysis.commands.len() != 1 {
        return false;
    }
    let invocation = &analysis.commands[0];
    if invocation.argv.first().map(String::as_str) != Some("rm")
        || !invocation.redirects.is_empty()
        || !invocation.env.is_empty()
    {
        return false;
    }
    let mut after_options = false;
    let mut paths = 0;
    for argument in &invocation.argv[1..] {
        if !after_options {
            if argument == "--" {
                after_options = true;
                continue;
            }
            if argument.starts_with('-') {
                static RECURSIVE: std::sync::LazyLock<regex::Regex> =
                    std::sync::LazyLock::new(|| regex::Regex::new(r"^-[a-zA-Z]*[rR]").unwrap());
                if argument == "--recursive" || RECURSIVE.is_match(argument) {
                    return false;
                }
                continue;
            }
        }
        if argument.contains(['*', '?', '['])
            || !is_absolute(argument)
            || !argument.ends_with(".md")
            || contained_segments(root, argument, cwd).is_none()
        {
            return false;
        }
        paths += 1;
    }
    paths > 0
}

/// 提取判定所需的消息摘要：真实用户输入是否含 ≥3 个词；assistant 的 Write/Edit 目标路径。
pub enum DecisionMessage {
    User { prose: bool },
    Assistant { writes: Vec<String> },
    Other,
}
pub enum Decision {
    Run(usize),
    Skip,
}
/// TS evaluateMemoryExtraction：`cursor` 为上次推进到的消息下标（不存在则全部视为新消息）。
pub fn decide(
    messages: &[DecisionMessage],
    cursor: Option<usize>,
    root: &str,
    cwd: &str,
) -> Decision {
    let after = match cursor {
        None => Some(messages),
        // 游标消息已不在（回退/改写）时视为未找到：与 TS messagesAfterFoundCursor 相同。
        Some(index) => messages.get(index + 1..),
    };
    let count = after.map_or(messages.len(), <[DecisionMessage]>::len);
    let direct_write = after.is_some_and(|after| {
        after.iter().any(|m| match m {
            DecisionMessage::Assistant { writes } => writes
                .iter()
                .any(|path| contained_segments(root, path, cwd).is_some()),
            _ => false,
        })
    });
    if direct_write {
        return Decision::Skip;
    }
    let prose = after
        .unwrap_or(messages)
        .iter()
        .any(|m| matches!(m, DecisionMessage::User { prose: true }));
    if prose {
        Decision::Run(count)
    } else {
        Decision::Skip
    }
}
/// TS countWords ≥ 3（按任意空白切分）。
pub fn is_prose(text: &str) -> bool {
    text.split(|c: char| c.is_whitespace() || c == '\u{feff}')
        .filter(|w| !w.is_empty())
        .count()
        >= 3
}

/// 主会话写记忆 Markdown 时放行（TS applyMemoryFilePermission）：目标必须落在记忆根内；
/// 保留非 plan 只读的 deny 与项目/Hook 的显式 ask。
pub fn permission_override(
    decision: super::permission::Decision,
    tool: &str,
    input: &Value,
    root: Option<&str>,
    cwd: &str,
) -> super::permission::Decision {
    use super::permission::{Behavior, Decision};
    let (Some(root), Some(path)) = (root, input["file_path"].as_str().filter(|p| !p.is_empty()))
    else {
        return decision;
    };
    if !matches!(tool, "Write" | "Edit") || contained_segments(root, path, cwd).is_none() {
        return decision;
    }
    let preserved = match decision.behavior {
        Behavior::Deny => decision.rule_id != "mode.plan.nonReadOnly",
        Behavior::Ask => matches!(decision.rule_id, "rule.project.ask" | "hook.PreToolUse.ask"),
        Behavior::Allow => false,
    };
    if preserved || !is_safe_memory_markdown(root, path, cwd) {
        return decision;
    }
    Decision {
        behavior: Behavior::Allow,
        rule_id: "memory.file.markdown",
    }
}
