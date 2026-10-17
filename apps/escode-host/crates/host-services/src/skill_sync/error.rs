//! `skill-sync` 的错误形状（docs/specs/rust-u2-sync.md §1.1）。
//!
//! 普通错误对应 TS 的 `Error`；超限错误保留稳定 `code` 与结构化 `data`，
//! UI 据此按当前语言展示可操作提示（TS `SkillSyncSizeLimitError`）。

use serde_json::{Value, json};

pub const SIZE_LIMIT_CODE: &str = "SKILL_SYNC_SIZE_LIMIT_EXCEEDED";

#[derive(Debug)]
pub struct SkillSyncError {
    pub message: String,
    pub size_limit: Option<(u64, u64, &'static str)>,
}

impl SkillSyncError {
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            size_limit: None,
        }
    }

    /// TS `createSkillSyncSizeLimitError`。
    pub fn size_limit(actual: u64, max: u64, phase: &'static str) -> Self {
        Self {
            message: format!("skill sync size limit exceeded: {actual}/{max} ({phase})"),
            size_limit: Some((actual, max, phase)),
        }
    }

    pub fn code(&self) -> Option<&'static str> {
        self.size_limit.map(|_| SIZE_LIMIT_CODE)
    }

    pub fn data(&self) -> Option<Value> {
        self.size_limit.map(
            |(actual, max, phase)| json!({"actualBytes": actual, "maxBytes": max, "phase": phase}),
        )
    }
}

impl From<std::io::Error> for SkillSyncError {
    fn from(error: std::io::Error) -> Self {
        Self::plain(error.to_string())
    }
}
