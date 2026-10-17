//! 命令来源描述表（TS `commandsService.ts` 的 `CommandAgentSourceDescriptor` 一族，spec §2）。
//!
//! 这张表决定每个来源的用户/工作区目录段、扩展名、命名空间分隔符与是否支持 `argument-hint`。
//! 本片只用到「用户目录」一列；列表来源表（在描述表之上追加 `agents` → `.agents/commands`）
//! 待到实现 `list` 时接入——**现在不预埋**，避免没有调用方的表项先漂移。

use std::path::{Path, PathBuf};

/// `escodeAgent`（`ESCODE_COMMAND_AGENT_SOURCES` 当前唯一成员，`escode-agent-policy.ts:22`）。
pub const AGENT_SOURCES: [&str; 1] = ["escodeAgent"];

pub struct SourceDescriptor {
    pub agent_source: &'static str,
    /// TS `directorySource`（settings 目录来源；`.escode` 与 `.agents` 两个来源共用描述表）。
    pub directory_source: &'static str,
    pub user_directory_segments: &'static [&'static str],
    #[allow(dead_code)] // 工作区目录段与以下三项随 `list`/写面接入（spec §1）
    pub workspace_directory_segments: &'static [&'static str],
    #[allow(dead_code)]
    pub file_extension: &'static str,
    #[allow(dead_code)]
    pub namespace_separator: &'static str,
    #[allow(dead_code)]
    pub supports_argument_hint: bool,
}

pub const ESCODE_AGENT: SourceDescriptor = SourceDescriptor {
    agent_source: "escodeAgent",
    directory_source: "escode",
    user_directory_segments: &[".escode", "commands"],
    workspace_directory_segments: &[".escode", "commands"],
    file_extension: ".md",
    namespace_separator: "/",
    supports_argument_hint: true,
};

/// TS `DEFAULT_COMMAND_AGENT_SOURCE`。
pub const DEFAULT_AGENT_SOURCE: &str = "escodeAgent";

impl SourceDescriptor {
    /// `join(home, ...userDirectorySegments)`。
    pub fn user_directory(&self, home: &Path) -> PathBuf {
        let mut path = home.to_path_buf();
        for segment in self.user_directory_segments {
            path.push(segment);
        }
        path
    }
}

/// 缺省 `escodeAgent`；**未知来源报错**（TS 在描述表里查不到时会读 `undefined.userDirectorySegments` 而抛错）。
pub fn descriptor_for(agent_source: Option<&str>) -> Result<&'static SourceDescriptor, String> {
    let requested = agent_source.unwrap_or(DEFAULT_AGENT_SOURCE);
    match requested {
        "escodeAgent" => Ok(&ESCODE_AGENT),
        other => Err(format!("Unknown command agent source: {other}")),
    }
}

#[cfg(test)]
#[path = "descriptors_tests.rs"]
mod tests;
