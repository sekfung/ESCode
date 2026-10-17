//! `settings-sync` 域（docs/specs/rust-u2-sync.md §3）。
//!
//! 本片交付 4/6：Claude `CLAUDE.md` → `.escode/AGENTS.md` 迁移（状态/复制）与首启提示状态。
//! `detect` / `importSelected` 是多 agent 设置发现/导入引擎，留待下一片（RPC 层回显缺口）。

mod agents_file;
mod first_run;

use std::path::PathBuf;

pub use agents_file::{AgentsFileCopyResult, AgentsFileMigrationStatus};

#[derive(Clone)]
pub struct SettingsSyncService {
    home: PathBuf,
}

impl SettingsSyncService {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    pub async fn agents_file_migration_status(&self) -> Result<AgentsFileMigrationStatus, String> {
        agents_file::migration_status(&self.home).await
    }

    pub async fn copy_agents_file(&self, overwrite: bool) -> Result<AgentsFileCopyResult, String> {
        agents_file::copy(&self.home, overwrite).await
    }

    pub async fn first_run_prompt_state(&self) -> Result<serde_json::Value, String> {
        first_run::state(&self.home).await
    }

    pub async fn mark_first_run_prompt_handled(&self) -> Result<(), String> {
        first_run::mark_handled(&self.home).await
    }
}
