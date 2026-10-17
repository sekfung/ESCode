//! `commands` 域（docs/specs/rust-u2-commands.md）。
//!
//! 本片交付 1/6：`getPrimaryUserCommandsDirectory`，并建立其余 5 个成员都要用的**命令来源描述表**
//! （TS `CommandAgentSourceDescriptor`）。当前只有一个合法来源 `escodeAgent`。

pub mod descriptors;

use std::path::Path;

pub use descriptors::{AGENT_SOURCES, SourceDescriptor, descriptor_for};

/// 解析「首要用户命令目录」并确保它落盘（TS `getPrimaryUserCommandsDirectory`）。
///
/// `home` = Host 的设置域根（TS `HOME || USERPROFILE || os.homedir()`；Main 经 `rpcListen.home` 提交同值）。
/// **必须建目录**：Windows 的文件管理器打不开不存在的路径，所以先把目录落盘再把路径交给系统。
pub async fn primary_user_commands_directory(
    home: &Path,
    agent_source: Option<&str>,
) -> Result<String, String> {
    let descriptor = descriptor_for(agent_source)?;
    let path = descriptor.user_directory(home);
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|error| format!("{error}"))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
