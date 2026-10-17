//! Claude `CLAUDE.md` → `.escode/AGENTS.md` 迁移（TS `getClaudeAgentsFileMigrationStatus` / `copy…`）。
//!
//! 存在性用 `lstat`（软链本身也算存在，含悬空软链）；复制跟随源软链并覆盖目标。

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub struct AgentsFileMigrationStatus {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub source_exists: bool,
    pub target_exists: bool,
}

impl AgentsFileMigrationStatus {
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "sourcePath": self.source_path.to_string_lossy(),
            "targetPath": self.target_path.to_string_lossy(),
            "sourceExists": self.source_exists,
            "targetExists": self.target_exists,
            "supported": self.source_exists,
        });
        if !self.source_exists {
            value["unavailableReason"] = Value::from("missingSource");
        }
        value
    }
}

pub struct AgentsFileCopyResult {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub status: &'static str,
    pub overwritten: bool,
    pub skipped_reason: Option<&'static str>,
}

impl AgentsFileCopyResult {
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "sourcePath": self.source_path.to_string_lossy(),
            "targetPath": self.target_path.to_string_lossy(),
            "status": self.status,
            "overwritten": self.overwritten,
        });
        if let Some(reason) = self.skipped_reason {
            value["skippedReason"] = Value::from(reason);
        }
        value
    }
}

fn source_path(home: &Path) -> PathBuf {
    home.join(".claude").join("CLAUDE.md")
}

fn target_path(home: &Path) -> PathBuf {
    home.join(".escode").join("AGENTS.md")
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

pub(super) async fn migration_status(home: &Path) -> Result<AgentsFileMigrationStatus, String> {
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || {
        Ok(AgentsFileMigrationStatus {
            source_path: source_path(&home),
            target_path: target_path(&home),
            source_exists: path_exists(&source_path(&home)),
            target_exists: path_exists(&target_path(&home)),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) async fn copy(home: &Path, overwrite: bool) -> Result<AgentsFileCopyResult, String> {
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || copy_sync(&home, overwrite))
        .await
        .map_err(|error| error.to_string())?
}

fn copy_sync(home: &Path, overwrite: bool) -> Result<AgentsFileCopyResult, String> {
    let source = source_path(home);
    let target = target_path(home);
    let source_exists = path_exists(&source);
    let target_exists = path_exists(&target);
    if !source_exists {
        return Ok(AgentsFileCopyResult {
            source_path: source,
            target_path: target,
            status: "skipped",
            overwritten: false,
            skipped_reason: Some("missingSource"),
        });
    }
    if target_exists && !overwrite {
        return Ok(AgentsFileCopyResult {
            source_path: source,
            target_path: target,
            status: "skipped",
            overwritten: false,
            skipped_reason: Some("targetExists"),
        });
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::copy(&source, &target).map_err(|error| error.to_string())?;
    Ok(AgentsFileCopyResult {
        source_path: source,
        target_path: target,
        status: "copied",
        overwritten: target_exists,
        skipped_reason: None,
    })
}

#[cfg(test)]
#[path = "agents_file_tests.rs"]
mod tests;
