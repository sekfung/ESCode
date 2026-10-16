//! Git 提交消息的 prompt 构造与输出校验（TS `gitCommitMessageGenerator.ts` 的纯函数部分）。
//! JS 的 `.length` / `slice` 以 UTF-16 码元计，这里的截断逐字复刻该口径。

use serde_json::Value;

use super::paths::js_trim;

const MAX_PROMPT_FILES: usize = 20;
const MAX_DIFF_FILES: usize = 8;
const MAX_DIFF_CHARS: usize = 12_000;
const MAX_DIFF_CHARS_PER_FILE: usize = 2_000;
const MAX_CONVERSATION_CONTEXT_MESSAGES: usize = 12;
const MAX_CONVERSATION_CONTEXT_CHARS: usize = 4_000;
const MAX_CONVERSATION_CONTEXT_CHARS_PER_MESSAGE: usize = 600;
const MAX_COMMIT_MESSAGE_CHARS: usize = 1_000;
const TYPES: [&str; 10] = [
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
];

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `String.prototype.slice(0, n)`：按 UTF-16 码元取前缀（落在代理对中间时退到上一个字符边界，JS 会留下孤立代理项，
/// 此处无法表示，取更保守的完整字符）。
fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        let next = count + ch.len_utf16();
        if next > units {
            return &text[..index];
        }
        count = next;
    }
    text
}

fn trim_end(text: &str) -> &str {
    super::paths::js_trim_end(text)
}

/// TS `clipText`。
fn clip_text(value: &str, max_chars: usize, marker: &str) -> String {
    if utf16_len(value) <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(utf16_len(marker) + 1);
    format!("{}\n{marker}", trim_end(utf16_prefix(value, keep)))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn build_diff_summary(diffs: &[Value]) -> String {
    let mut total = 0usize;
    let mut chunks: Vec<String> = Vec::new();
    for diff in diffs.iter().take(MAX_DIFF_FILES) {
        let header = format!("--- {}", text(diff, "path"));
        let patch = js_trim(text(diff, "patch"));
        let summary = js_trim(text(diff, "summary"));
        let body = if !patch.is_empty() {
            patch
        } else if !summary.is_empty() {
            summary
        } else {
            "(diff unavailable)"
        };
        let remaining = MAX_DIFF_CHARS as i64 - total as i64;
        if remaining <= 0 {
            break;
        }
        let clipped = clip_text(
            body,
            MAX_DIFF_CHARS_PER_FILE.min(remaining as usize),
            "...diff truncated...",
        );
        let chunk = format!("{header}\n{clipped}");
        total += utf16_len(&chunk);
        chunks.push(chunk);
    }
    chunks.join("\n\n")
}

/// TS `normalizeConversationContextText`：统一换行、去行尾空白、压缩 3+ 空行。
fn normalize_context_text(value: &str) -> String {
    let unified = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = Vec::new();
    for line in unified.split('\n') {
        lines.push(line);
    }
    // `[ \t]+\n` → `\n`：只去掉紧贴换行的空格/制表符。
    let mut joined = String::new();
    for (index, line) in lines.iter().enumerate() {
        let is_last = index + 1 == lines.len();
        joined.push_str(if is_last {
            line
        } else {
            line.trim_end_matches([' ', '\t'])
        });
        if !is_last {
            joined.push('\n');
        }
    }
    // `\n{3,}` → `\n\n`
    let mut out = String::new();
    let mut newlines = 0;
    for ch in joined.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(ch);
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    js_trim(&out).to_owned()
}

fn build_conversation_summary(context: &Value) -> String {
    let messages: &[Value] = context
        .get("messages")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    if messages.is_empty() {
        return String::new();
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut total = 0usize;
    let omitted = context
        .get("omittedMessageCount")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
        + messages
            .len()
            .saturating_sub(MAX_CONVERSATION_CONTEXT_MESSAGES);
    if omitted > 0 {
        chunks.push(format!("- {omitted} earlier messages omitted"));
    }
    let start = messages
        .len()
        .saturating_sub(MAX_CONVERSATION_CONTEXT_MESSAGES);
    for message in &messages[start..] {
        let content = normalize_context_text(text(message, "content"));
        if content.is_empty() {
            continue;
        }
        let role = if text(message, "role") == "assistant" {
            "Assistant"
        } else {
            "User"
        };
        let clipped = clip_text(
            &content,
            MAX_CONVERSATION_CONTEXT_CHARS_PER_MESSAGE,
            "...message truncated...",
        );
        let next = format!("{role}: {clipped}");
        let remaining = MAX_CONVERSATION_CONTEXT_CHARS as i64 - total as i64;
        if remaining <= 0 {
            break;
        }
        let chunk = clip_text(&next, remaining as usize, "...conversation truncated...");
        total += utf16_len(&chunk) + 1;
        chunks.push(chunk);
    }
    chunks.join("\n")
}

/// `locale` 缺省时由调用方给出运行时 locale。
pub fn build_prompt(
    branch_name: Option<&str>,
    locale: Option<&str>,
    runtime_locale: Option<&str>,
    files: &[Value],
    diffs: &[Value],
    conversation_context: &Value,
) -> String {
    let branch = branch_name
        .map(js_trim)
        .filter(|b| !b.is_empty())
        .unwrap_or("(detached or unknown)");
    let candidate = locale.or(runtime_locale);
    let language = if candidate.is_some_and(|l| l.to_lowercase().starts_with("zh")) {
        "Chinese"
    } else {
        "English"
    };
    let visible = files.iter().take(MAX_PROMPT_FILES);
    let omitted = files.len().saturating_sub(MAX_PROMPT_FILES);
    let file_summary = visible
        .map(|file| {
            format!(
                "- {} {} (+{}/-{})",
                text(file, "kind"),
                text(file, "repoRelativePath"),
                file["added"].as_u64().unwrap_or(0),
                file["removed"].as_u64().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let diff_summary = build_diff_summary(diffs);
    let conversation = build_conversation_summary(conversation_context);
    let or = |value: String, fallback: &str| {
        if value.is_empty() {
            fallback.to_owned()
        } else {
            value
        }
    };
    let omitted_line = if omitted > 0 {
        format!("- ... {omitted} more files")
    } else {
        String::new()
    };
    [
        "Write exactly one Git commit message for the workspace changes below.".to_owned(),
        "Return only the commit message text.".to_owned(),
        String::new(),
        "Hard requirements:".to_owned(),
        "- The first line must be a valid Conventional Commit subject.".to_owned(),
        "- Use one of: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert."
            .to_owned(),
        "- Keep the Conventional Commit type and optional scope in English.".to_owned(),
        "- Write the subject and any body in the current language.".to_owned(),
        "- Keep the subject under 72 characters.".to_owned(),
        "- Use the current session conversation context only to infer user intent.".to_owned(),
        "- Do not mention the conversation, chat, prompt, or user request explicitly.".to_owned(),
        "- Do not explain your reasoning.".to_owned(),
        "- Do not repeat these instructions.".to_owned(),
        String::new(),
        format!("Current branch: {branch}"),
        format!("Current language: {language}"),
        String::new(),
        "Current session conversation context:".to_owned(),
        or(conversation, "(not provided)"),
        String::new(),
        "Changed files:".to_owned(),
        or(file_summary, "- (no file summary available)"),
        omitted_line,
        String::new(),
        "Diff excerpts:".to_owned(),
        or(
            diff_summary,
            "(diff unavailable; infer only from the changed file summary)",
        ),
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

/// TS `stripGeneratedCommitMessageDecorations`。
fn strip_decorations(value: &str) -> String {
    let trimmed = js_trim(value);
    let fenced = trimmed
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
        .filter(|_| trimmed.len() >= 6)
        .map(|inner| {
            // 可选语言标记 `[a-zA-Z0-9_-]*` 后接空白，再取到结尾空白前的正文。
            let after_lang = inner
                .trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            // 语言标记必须紧跟空白或直接是正文开头，与正则 `(?:[a-zA-Z0-9_-]+)?\s*` 等价。
            js_trim(
                if after_lang.len() == inner.len() || after_lang.starts_with(char::is_whitespace) {
                    after_lang
                } else {
                    inner
                },
            )
        });
    let without_fence = fenced.unwrap_or(trimmed);
    let lower = without_fence.to_lowercase();
    let without_prefix = if lower.starts_with("commit message:") {
        js_trim(&without_fence["commit message:".len()..])
    } else {
        without_fence
    };
    let quoted = (without_prefix.starts_with('"') && without_prefix.ends_with('"'))
        || (without_prefix.starts_with('\'') && without_prefix.ends_with('\''));
    if quoted && without_prefix.len() >= 2 {
        return without_prefix[1..without_prefix.len() - 1].to_owned();
    }
    without_prefix.to_owned()
}

fn is_conventional_subject(subject: &str) -> bool {
    let Some((head, rest)) = subject.split_once(": ") else {
        return false;
    };
    let mut head = head;
    if let Some(stripped) = head.strip_suffix('!') {
        head = stripped;
    }
    let (kind, scope_ok) = match head.find('(') {
        Some(open) => {
            let scope = &head[open..];
            (
                &head[..open],
                scope.len() > 2 && scope.ends_with(')') && !scope[1..scope.len() - 1].contains(')'),
            )
        }
        None => (head, true),
    };
    let kind_ok = TYPES.contains(&kind) || kind == "revert";
    let units = utf16_len(rest);
    let body_ok =
        (1..=100).contains(&units) && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    kind_ok && scope_ok && body_ok
}

pub enum Validation {
    Ok(String),
    Invalid { preview: Option<String> },
}

/// TS `validateGeneratedGitCommitMessage`。
pub fn validate(raw: &str) -> Validation {
    let stripped = strip_decorations(raw);
    let message = js_trim(utf16_prefix(js_trim(&stripped), MAX_COMMIT_MESSAGE_CHARS)).to_owned();
    if message.is_empty() {
        return Validation::Invalid { preview: None };
    }
    let first_line = message.split('\n').next().unwrap_or_default();
    let subject = js_trim(first_line.strip_suffix('\r').unwrap_or(first_line));
    if !is_conventional_subject(subject) {
        let preview = if subject.is_empty() {
            utf16_prefix(&message, 120).to_owned()
        } else {
            subject.to_owned()
        };
        return Validation::Invalid {
            preview: Some(preview),
        };
    }
    Validation::Ok(message)
}

/// 生成失败：`message` 是给用户的文案，`reason`/`detail` 对应 TS `GitCommitMessageGenerationError`。
#[derive(Debug)]
pub struct Failure {
    pub message: &'static str,
    pub reason: &'static str,
    pub detail: Option<String>,
}

/// TS `normalizeModelText` + `validateGeneratedGitCommitMessage` + 失败包装。
pub fn finalize_model_text(raw: &str) -> Result<String, Failure> {
    let text = js_trim(raw);
    if text.is_empty() {
        return Err(Failure {
            message: "模型请求失败。",
            reason: "request-failed",
            detail: Some("模型响应缺少文本内容。".to_owned()),
        });
    }
    match validate(text) {
        Validation::Ok(message) => Ok(message),
        Validation::Invalid { preview } => Err(Failure {
            message: "模型没有返回可用的 Conventional Commit 提交消息。",
            reason: "invalid-output",
            detail: preview,
        }),
    }
}

#[cfg(test)]
#[path = "commit_message_prompt_tests.rs"]
mod tests;
