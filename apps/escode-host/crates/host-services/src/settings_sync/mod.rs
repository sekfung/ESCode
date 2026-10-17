//! `settings-sync` 域（docs/specs/rust-u2-sync.md §3）。
//!
//! 六个成员：Claude `CLAUDE.md` → `.escode/AGENTS.md` 迁移（状态/复制）、首启提示状态，
//! 以及多 agent 设置发现/导入（`detect` / `importSelected`）。

mod agents;
mod agents_file;
mod candidates;
mod discovery;
mod first_run;
mod import;
mod mcp_source;
mod metadata;
mod sources;
mod sources_tables;

use std::path::PathBuf;

use crate::skills::Compare as SkillsCompare;
pub use agents_file::{AgentsFileCopyResult, AgentsFileMigrationStatus};

#[derive(Clone)]
pub struct SettingsSyncService {
    home: PathBuf,
    compare: SkillsCompare,
}

impl SettingsSyncService {
    pub fn new(home: PathBuf, compare: SkillsCompare) -> Self {
        Self { home, compare }
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

    pub async fn detect(&self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        let service = self.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let workspace = request["workspacePath"]
                .as_str()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from);
            Ok(discovery::detect(
                &service.home,
                workspace.as_deref(),
                &request,
                &service.compare,
            ))
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub async fn import_selected(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let service = self.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let workspace = request["workspacePath"]
                .as_str()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from);
            Ok(import::import_selected(
                &service.home,
                workspace.as_deref(),
                &request,
                &service.compare,
            ))
        })
        .await
        .map_err(|error| error.to_string())?
    }
}
