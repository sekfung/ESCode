//! Claude Code jsonl 的头部解析（TS `claudeNativeSessionHeadParser.ts`）。
//!
//! 只读前 N 行：判定 sidechain、提取 cwd / 首条可见用户文本 / 创建时间。

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::parser::{
    extract_assistant_text, extract_user_text, read_entry_timestamp, read_workspace_path,
};

/// 扫描/查找时读取的行数（TS `readJsonLinesFileHead(filePath, 16)`）。
pub(super) const HEAD_LINES: usize = 16;

#[derive(Debug, Clone, Default)]
pub(super) struct HeadInfo {
    pub workspace_path: Option<String>,
    pub preview_title: Option<String>,
    pub created_at: Option<i64>,
}

pub(super) fn has_sidechain_marker(entries: &[Value]) -> bool {
    entries.iter().any(|entry| {
        entry["isSidechain"] == true
            || entry["message"]["isSidechain"] == true
            || entry["request"]["isSidechain"] == true
    })
}

pub(super) fn extract_head_info(entries: &[Value]) -> HeadInfo {
    let mut workspace_path = None;
    for entry in entries {
        let user = extract_user_text(entry);
        let assistant = extract_assistant_text(entry);
        if workspace_path.is_none() && (user.is_some() || assistant.is_some()) {
            workspace_path = read_workspace_path(entry);
        }
        if let Some(user) = user {
            return HeadInfo {
                workspace_path,
                preview_title: Some(derive_title(&user)),
                created_at: read_entry_timestamp(entry),
            };
        }
    }
    HeadInfo {
        workspace_path,
        ..HeadInfo::default()
    }
}

pub(super) fn derive_title(content: &str) -> String {
    if content.chars().count() > 50 {
        format!("{}...", content.chars().take(50).collect::<String>())
    } else {
        content.to_owned()
    }
}

/// TS `sanitizeClaudeVisibleText`：剥 IDE 注入与本地命令标签，统一换行后 trim。
pub(super) fn sanitize_visible_text(text: &str) -> String {
    static IDE: OnceLock<Regex> = OnceLock::new();
    static COMMAND: OnceLock<Regex> = OnceLock::new();
    let ide = IDE
        .get_or_init(|| Regex::new(r"(?is)<ide_opened_file>.*?</ide_opened_file>").expect("regex"));
    let command = COMMAND.get_or_init(|| {
        Regex::new(r"(?is)<(?:local-command|command)-[^>]+>.*?</(?:local-command|command)-[^>]+>")
            .expect("regex")
    });
    let text = ide.replace_all(text, " ");
    let text = command.replace_all(&text, " ");
    text.replace("\r\n", "\n").trim().to_owned()
}

#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
