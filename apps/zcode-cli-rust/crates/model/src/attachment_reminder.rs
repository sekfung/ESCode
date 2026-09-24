//! 文本附件进入模型请求的形态，对齐 TS `system-reminder/prompt-attachment.ts`：
//! 本地文本文件伪装成一次 Read 调用结果，作为独立的 `<system-reminder>` user 消息放在用户正文之前。
//! 见 docs/specs/rust-attachment-prompt.md。
use serde_json::{Value, json};

/// TS `READ_MAX_FILE_SIZE_BYTES`：超过即只读前 `READ_DEFAULT_MAX_LINES` 行。
pub(super) const READ_MAX_FILE_SIZE_BYTES: usize = 256 * 1024;
/// TS `READ_DEFAULT_MAX_LINES`。
pub(super) const READ_DEFAULT_MAX_LINES: usize = 2_000;

/// 返回 reminder 正文（未包裹标签）。`label` 是用户提交的附件引用（TS `source.text.value`）。
pub(super) fn read_like_body(label: &str, text: &str) -> String {
    let label = sanitize_label(label);
    let (content, truncated) = if text.len() > READ_MAX_FILE_SIZE_BYTES {
        (first_lines(text, READ_DEFAULT_MAX_LINES), true)
    } else {
        (text, false)
    };
    let mut bodies = vec![
        format!(
            "Called the Read tool with the following input: {}",
            json!({ "file_path": label })
        ),
        format!("Result of calling the Read tool:\n{}", read_output(content)),
    ];
    if truncated {
        bodies.push(format!(
            "Note: The file {label} was too large and has been truncated to the first {READ_DEFAULT_MAX_LINES} lines. Don't tell the user about this truncation. Use Read to read more of the file if you need."
        ));
    }
    bodies.join("\n")
}

/// TS `wrapSystemReminderForSource("prompt_attachment", body)`：中和嵌套标签后按行包裹，无尾随换行。
pub(super) fn wrap(body: &str) -> String {
    format!(
        "<system-reminder>\n{}\n</system-reminder>",
        escape_nested(body)
    )
}

pub(super) fn reminder_message(label: &str, text: &str) -> Value {
    json!({"role": "user", "content": wrap(&read_like_body(label, text))})
}

/// TS `formatReadTextOutput`（无 offset、无部分视图）：逐行编号（按 /\r?\n/ 切分）。
fn read_output(content: &str) -> String {
    if content.is_empty() {
        // 与 TS 实测一致（差分用例 empty）：TS 读取器对空文件报告 totalLines=1，
        // formatReadTextOutput 因此走「短于 offset」分支，而不是 EMPTY_FILE_REMINDER。
        return "<system-reminder>Warning: the file exists but is shorter than the provided offset (1). The file has 1 lines.</system-reminder>"
            .to_owned();
    }
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .enumerate()
        .map(|(i, line)| format!("{}\t{line}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn first_lines(text: &str, limit: usize) -> &str {
    match text.match_indices('\n').nth(limit - 1) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// TS `sanitizeAttachmentLabel`：折叠空白、去首尾，超过 200 字符截为 197 + "..."。
fn sanitize_label(label: &str) -> String {
    let normalized = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() > 200 {
        format!("{}...", chars[..197].iter().collect::<String>())
    } else {
        normalized
    }
}

/// TS `escapeNestedSystemReminderTags`：`/<\/?system-reminder\b/gi` 的起始 `<` 改为 `&lt;`。
fn escape_nested(body: &str) -> String {
    const TAG: &str = "system-reminder";
    let lower = body.to_ascii_lowercase();
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if bytes[i] == b'<' {
            let after = if lower[i + 1..].starts_with('/') {
                i + 2
            } else {
                i + 1
            };
            let end = after + TAG.len();
            let boundary = lower
                .get(end..)
                .and_then(|rest| rest.chars().next())
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
            if lower.get(after..end) == Some(TAG) && boundary {
                out.push_str("&lt;");
                i += 1;
                continue;
            }
        }
        let ch = body[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lines_like_ts_and_keeps_the_trailing_empty_line() {
        assert_eq!(
            read_like_body("C:/w/notes.txt", "attached body\n"),
            "Called the Read tool with the following input: {\"file_path\":\"C:/w/notes.txt\"}\nResult of calling the Read tool:\n1\tattached body\n2\t"
        );
    }

    #[test]
    fn escapes_nested_reminder_tags_case_insensitively() {
        assert_eq!(
            escape_nested("a </System-Reminder> b <system-reminderx>"),
            "a &lt;/System-Reminder> b <system-reminderx>"
        );
    }

    #[test]
    fn long_labels_are_cut_like_ts() {
        let label = "x".repeat(250);
        assert_eq!(sanitize_label(&label).chars().count(), 200);
    }
}
