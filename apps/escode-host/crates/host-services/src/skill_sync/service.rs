//! `skill-sync` 服务（TS `createSkillSyncService`）：候选、远端状态、导出/导入与写权限探测。
//!
//! 所有文件操作在 `spawn_blocking` 里跑；服务不持有可变状态，唯一事实是用户级 skills 目录。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::archive::{ArchiveEntry, create_archive};
use super::candidates::{self, Candidate};
use super::error::SkillSyncError;
use super::import::import_archive;
use super::paths;
use crate::skills::Compare;

/// TS `DEFAULT_MAX_ARCHIVE_BYTES`。
pub const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 20 * 1024 * 1024;
const UNSAFE_PATH: &str = "unsafe skill sync path";

#[derive(Clone)]
pub struct SkillSyncService {
    home: PathBuf,
    compare: Compare,
    max_archive_bytes: u64,
}

#[derive(Debug)]
pub struct ExportResult {
    pub archive: Vec<u8>,
    pub archive_bytes: u64,
    pub skills: Vec<Value>,
}

impl SkillSyncService {
    pub fn new(home: PathBuf, compare: Compare) -> Self {
        Self {
            home,
            compare,
            max_archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
        }
    }

    /// 测试用：缩小上限以覆盖三个阶段。
    pub fn with_max_archive_bytes(mut self, max: u64) -> Self {
        self.max_archive_bytes = max;
        self
    }

    pub async fn list_local_user_skill_candidates(&self) -> Result<Value, SkillSyncError> {
        let service = self.clone();
        blocking(move || {
            let candidates = candidates::collect(&service.home, &service.compare)?;
            Ok(json!({
                "candidates": candidates.iter().map(Candidate::to_json).collect::<Vec<_>>(),
                "maxArchiveBytes": service.max_archive_bytes,
            }))
        })
        .await
    }

    pub async fn list_remote_user_skill_statuses(
        &self,
        params: &Value,
    ) -> Result<Value, SkillSyncError> {
        let requests = StatusRequests::parse(params)?;
        let service = self.clone();
        blocking(move || service.remote_statuses(&requests)).await
    }

    pub async fn export_skills_archive(
        &self,
        params: &Value,
    ) -> Result<ExportResult, SkillSyncError> {
        let ids: Vec<String> = params["skillIds"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let service = self.clone();
        blocking(move || service.export(&ids)).await
    }

    pub async fn check_remote_user_skill_write_access(&self) -> Result<Value, SkillSyncError> {
        let service = self.clone();
        blocking(move || Ok(service.write_access())).await
    }

    pub async fn import_skills_archive(
        &self,
        archive: Vec<u8>,
        overwrite: bool,
    ) -> Result<Value, SkillSyncError> {
        let service = self.clone();
        blocking(move || {
            if overwrite {
                return Err(SkillSyncError::plain(
                    "skill sync overwrite is not supported",
                ));
            }
            if archive.len() as u64 > service.max_archive_bytes {
                return Err(SkillSyncError::size_limit(
                    archive.len() as u64,
                    service.max_archive_bytes,
                    "archive",
                ));
            }
            import_archive(
                &service.home,
                &service.compare,
                &archive,
                service.max_archive_bytes,
            )
        })
        .await
    }

    fn remote_statuses(&self, requests: &StatusRequests) -> Result<Value, SkillSyncError> {
        let root = candidates::user_escode_root(&self.home);
        let existing = candidates::directory_path_by_name(&self.home)?;
        let mut statuses = Vec::with_capacity(requests.directory_names.len());
        for requested in &requests.directory_names {
            let directory_name = paths::normalize_relative(requested, UNSAFE_PATH)?;
            let target = paths::resolve_within(&root, &directory_name, UNSAFE_PATH)?;
            if target.exists() {
                statuses.push(json!({
                    "directoryName": directory_name,
                    "exists": true,
                    "path": target.to_string_lossy(),
                }));
                continue;
            }
            let existing_path = requests
                .names_by_directory
                .get(&directory_name)
                .map(|name| candidates::name_key(name))
                .and_then(|key| existing.get(&key).cloned());
            match existing_path {
                Some(path) => statuses.push(json!({
                    "directoryName": directory_name,
                    "exists": true,
                    "path": path.to_string_lossy(),
                })),
                None => statuses.push(json!({
                    "directoryName": directory_name,
                    "exists": false,
                })),
            }
        }
        Ok(json!({"statuses": statuses}))
    }

    fn export(&self, ids: &[String]) -> Result<ExportResult, SkillSyncError> {
        let candidates = candidates::collect(&self.home, &self.compare)?;
        let by_id: BTreeMap<&str, &Candidate> = candidates
            .iter()
            .map(|candidate| (candidate.id.as_str(), candidate))
            .collect();
        let mut selected = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(candidate) = by_id.get(id.as_str()) else {
                return Err(SkillSyncError::plain(format!(
                    "skill sync candidate not found: {id}"
                )));
            };
            selected.push(*candidate);
        }
        let mut entries = Vec::with_capacity(selected.len());
        let mut selected_bytes = 0_u64;
        for candidate in &selected {
            let directory = candidate
                .path
                .parent()
                .ok_or_else(|| SkillSyncError::plain("invalid skill candidate path"))?;
            let source_path = std::fs::canonicalize(directory)?;
            selected_bytes = selected_bytes
                .checked_add(candidate.size_bytes)
                .ok_or_else(|| SkillSyncError::plain("invalid skill candidate size"))?;
            entries.push(ArchiveEntry {
                source_path,
                archive_path: candidate.directory_name.clone(),
            });
        }
        if selected_bytes > self.max_archive_bytes {
            return Err(SkillSyncError::size_limit(
                selected_bytes,
                self.max_archive_bytes,
                "selected-content",
            ));
        }
        let archive = create_archive(&entries, &self.compare)?;
        let archive_bytes = archive.len() as u64;
        if archive_bytes > self.max_archive_bytes {
            return Err(SkillSyncError::size_limit(
                archive_bytes,
                self.max_archive_bytes,
                "archive",
            ));
        }
        Ok(ExportResult {
            archive,
            archive_bytes,
            skills: selected
                .iter()
                .map(|candidate| {
                    json!({
                        "id": candidate.id,
                        "name": candidate.name,
                        "directoryName": candidate.directory_name,
                    })
                })
                .collect(),
        })
    }

    /// TS `checkRemoteSyncDirectoryWriteAccess`。
    fn write_access(&self) -> Value {
        let root = candidates::user_escode_root(&self.home);
        let marker = root.join(format!(
            ".escode-sync-preflight-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(&root)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)?;
            file.write_all(b"ok")?;
            std::fs::remove_file(&marker)?;
            Ok(())
        })();
        match result {
            Ok(()) => json!({"ok": true, "path": root.to_string_lossy()}),
            Err(error) => {
                let _ = std::fs::remove_file(&marker);
                json!({
                    "ok": false,
                    "path": root.to_string_lossy(),
                    "error": error.to_string(),
                })
            }
        }
    }
}

struct StatusRequests {
    directory_names: Vec<String>,
    names_by_directory: BTreeMap<String, String>,
}

impl StatusRequests {
    fn parse(params: &Value) -> Result<Self, SkillSyncError> {
        let directory_names = params["directoryNames"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let mut names_by_directory = BTreeMap::new();
        for skill in params["skills"].as_array().into_iter().flatten() {
            let directory = skill["directoryName"].as_str().unwrap_or_default();
            let name = skill["name"].as_str().unwrap_or_default();
            names_by_directory.insert(
                paths::normalize_relative(directory, UNSAFE_PATH)?,
                name.to_owned(),
            );
        }
        Ok(Self {
            directory_names,
            names_by_directory,
        })
    }
}

async fn blocking<T, F>(run: F) -> Result<T, SkillSyncError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SkillSyncError> + Send + 'static,
{
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| SkillSyncError::plain(error.to_string()))?
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
