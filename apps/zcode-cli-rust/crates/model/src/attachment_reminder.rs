//! 文本附件进入模型请求的形态，对齐 TS `system-reminder/prompt-attachment.ts`：
//! 本地文本文件伪装成一次 Read 调用结果，作为独立的 `<system-reminder>` user 消息放在用户正文之前。
//! 见 docs/specs/rust-attachment-prompt.md。
use serde_json::{Value, json};

/// 返回 reminder 正文（未包裹标签）。`label` 是用户提交的附件引用（TS `source.text.value`）。
/// TS 的截断说明只在 `truncated && !partialViewNotice` 时出现；读取器对整文件读取从不置 truncated，
/// 超限内容一律以部分视图提示表达，因此这里不再追加截断说明。
pub(super) fn read_like_body(label: &str, text: &str, size_bytes: usize) -> String {
    let label = sanitize_label(label);
    let read = super::attachment_read::read(text, size_bytes);
    [
        format!(
            "Called the Read tool with the following input: {}",
            json!({ "file_path": label })
        ),
        format!(
            "Result of calling the Read tool:\n{}",
            read_output(&read.content, read.partial_view_notice.as_deref())
        ),
    ]
    .join("\n")
}

/// TS `wrapSystemReminderForSource("prompt_attachment", body)`：中和嵌套标签后按行包裹，无尾随换行。
pub(super) fn wrap(body: &str) -> String {
    format!(
        "<system-reminder>\n{}\n</system-reminder>",
        escape_nested(body)
    )
}

pub(super) fn reminder_message(label: &str, text: &str, size_bytes: usize) -> Value {
    json!({"role": "user", "content": wrap(&read_like_body(label, text, size_bytes))})
}

/// TS `formatReadTextOutput`（从第 1 行起）：可选的部分视图提示 + 逐行编号（内容已 CRLF 归一）。
fn read_output(content: &str, partial_view_notice: Option<&str>) -> String {
    let prefix = partial_view_notice
        .map(|n| format!("<system-reminder>{n}</system-reminder>\n\n"))
        .unwrap_or_default();
    if content.is_empty() {
        // 与 TS 实测一致（差分用例 empty）：TS 读取器对空文件报告 totalLines=1，
        // formatReadTextOutput 因此走「短于 offset」分支，而不是 EMPTY_FILE_REMINDER。
        return format!(
            "{prefix}<system-reminder>Warning: the file exists but is shorter than the provided offset (1). The file has 1 lines.</system-reminder>"
        );
    }
    let numbered = content
        .split('\n')
        .enumerate()
        .map(|(i, line)| format!("{}\t{line}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{prefix}{numbered}")
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
            read_like_body("C:/w/notes.txt", "attached body\n", 14),
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
