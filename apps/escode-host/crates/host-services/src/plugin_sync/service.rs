//! plugin-sync 服务（TS `createPluginSyncService`）：7 个成员。
//!
//! 所有文件/归档操作在 `spawn_blocking` 中执行；Host 是用户级 plugin 配置与目录的唯一写者。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::ordered_json::{self, OrderedJson};
use crate::skills::Compare;

use super::archive::{ArchiveEntry, create_archive};
use super::candidates::{self, Candidate};
use super::{marketplace, paths};

pub const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Clone)]
pub struct PluginSyncService {
    home: PathBuf,
    compare: Compare,
    max_archive_bytes: u64,
}

pub struct ExportResult {
    pub archive: Vec<u8>,
    pub archive_bytes: u64,
    pub plugins: Vec<Value>,
}

pub struct MarketplaceExportResult {
    pub archive: Vec<u8>,
    pub archive_bytes: u64,
    pub marketplace_id: String,
    pub plugin_names: Vec<String>,
}

impl PluginSyncService {
    pub fn new(home: PathBuf, compare: Compare) -> Self {
        Self {
            home,
            compare,
            max_archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
        }
    }

    #[cfg(test)]
    pub fn with_max_archive_bytes(mut self, max: u64) -> Self {
        self.max_archive_bytes = max;
        self
    }

    pub async fn list_local_user_plugin_candidates(&self) -> Result<Value, String> {
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

    pub async fn list_remote_user_plugin_statuses(&self, params: &Value) -> Result<Value, String> {
        let requests: Vec<(String, String)> = params
            .get("plugins")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        (
                            item.get("pluginId")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            item.get("directoryName")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let service = self.clone();
        blocking(move || service.remote_statuses(&requests)).await
    }

    pub async fn export_plugins_archive(&self, params: &Value) -> Result<ExportResult, String> {
        let ids: Vec<String> = params
            .get("pluginIds")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let service = self.clone();
        blocking(move || service.export(&ids)).await
    }

    pub async fn import_plugins_archive(
        &self,
        archive: Vec<u8>,
        overwrite: bool,
    ) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            if overwrite {
                return Err("plugin sync overwrite is not supported".to_owned());
            }
            if archive.len() as u64 > service.max_archive_bytes {
                return Err(format!(
                    "plugin sync archive exceeds limit: {}/{}",
                    archive.len(),
                    service.max_archive_bytes
                ));
            }
            service.import(&archive)
        })
        .await
    }

    pub async fn check_remote_user_plugin_write_access(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || Ok(service.write_access())).await
    }

    pub async fn export_marketplace_source_archive(
        &self,
        params: &Value,
    ) -> Result<MarketplaceExportResult, String> {
        let service = self.clone();
        let params = params.clone();
        blocking(move || {
            let result = marketplace::export(
                &service.home,
                &params,
                service.max_archive_bytes,
                &service.compare,
            )?;
            Ok(MarketplaceExportResult {
                archive: result.archive,
                archive_bytes: result.bytes,
                marketplace_id: result.marketplace_id,
                plugin_names: result.plugin_names,
            })
        })
        .await
    }

    pub async fn import_marketplace_source_archive(
        &self,
        archive: Vec<u8>,
        overwrite: bool,
    ) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            super::marketplace_import::import(
                &service.home,
                &archive,
                service.max_archive_bytes,
                overwrite,
                &service.compare,
            )
        })
        .await
    }

    fn remote_statuses(&self, requests: &[(String, String)]) -> Result<Value, String> {
        let root = candidates::plugin_root(&self.home);
        let existing = candidates::configured_paths(&self.home)?;
        let mut statuses = Vec::with_capacity(requests.len());
        for (plugin_id, directory_name) in requests {
            let directory_name =
                paths::normalize_relative(directory_name, "unsafe plugin sync path")?;
            let plugin_id = candidates::normalize_plugin_id(plugin_id)?;
            let target = paths::resolve_within(&root, &directory_name, "unsafe plugin sync path")?;
            if target.exists() {
                statuses.push(json!({
                    "pluginId": plugin_id,
                    "directoryName": directory_name,
                    "exists": true,
                    "path": target.to_string_lossy(),
                    "reason": "targetExists",
                }));
                continue;
            }
            match existing.get(&plugin_id.to_lowercase()) {
                Some(path) => statuses.push(json!({
                    "pluginId": plugin_id,
                    "directoryName": directory_name,
                    "exists": true,
                    "path": path.to_string_lossy(),
                    "reason": "samePluginId",
                })),
                None => statuses.push(json!({
                    "pluginId": plugin_id,
                    "directoryName": directory_name,
                    "exists": false,
                })),
            }
        }
        Ok(json!({"statuses": statuses}))
    }

    fn export(&self, ids: &[String]) -> Result<ExportResult, String> {
        let candidates = candidates::collect(&self.home, &self.compare)?;
        let by_id: BTreeMap<&str, &Candidate> = candidates
            .iter()
            .map(|candidate| (candidate.id.as_str(), candidate))
            .collect();
        let mut selected = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(candidate) = by_id.get(id.as_str()) else {
                return Err(format!("plugin sync candidate not found: {id}"));
            };
            selected.push(*candidate);
        }
        let selected_bytes = selected.iter().try_fold(0u64, |total, candidate| {
            total
                .checked_add(candidate.size_bytes)
                .ok_or_else(|| "invalid plugin candidate size".to_owned())
        })?;
        if selected_bytes > self.max_archive_bytes {
            return Err(format!(
                "plugin sync archive exceeds limit: {selected_bytes}/{}",
                self.max_archive_bytes
            ));
        }
        let mut entries = Vec::with_capacity(selected.len());
        for candidate in &selected {
            let source_path =
                std::fs::canonicalize(&candidate.path).map_err(|error| error.to_string())?;
            entries.push(ArchiveEntry::Source {
                source_path,
                archive_path: candidate.directory_name.clone(),
            });
        }
        let metadata = metadata(&selected);
        let archive = create_archive(&entries, &metadata, &self.compare)?;
        if archive.len() as u64 > self.max_archive_bytes {
            return Err(format!(
                "plugin sync archive exceeds limit: {}/{}",
                archive.len(),
                self.max_archive_bytes
            ));
        }
        Ok(ExportResult {
            archive_bytes: archive.len() as u64,
            archive,
            plugins: selected
                .iter()
                .map(|candidate| candidate_json(candidate))
                .collect(),
        })
    }

    fn import(&self, archive: &[u8]) -> Result<Value, String> {
        super::import::import(&self.home, archive, self.max_archive_bytes)
    }

    fn write_access(&self) -> Value {
        let config_dir = candidates::config_path(&self.home)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.home.clone());
        let directories = [candidates::plugin_root(&self.home), config_dir];
        for directory in &directories {
            let result = crate::remote_sync::check_directory_write_access(directory);
            if result["ok"] != Value::Bool(true) {
                return result;
            }
        }
        json!({
            "ok": true,
            "path": directories
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

fn candidate_json(candidate: &Candidate) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".to_owned(), Value::String(candidate.id.clone()));
    object.insert("name".to_owned(), Value::String(candidate.name.clone()));
    object.insert(
        "pluginId".to_owned(),
        Value::String(candidate.plugin_id.clone()),
    );
    object.insert(
        "directoryName".to_owned(),
        Value::String(candidate.directory_name.clone()),
    );
    if let Some(enabled) = candidate.enabled_override {
        object.insert("enabled".to_owned(), Value::Bool(enabled));
    }
    Value::Object(object)
}

async fn blocking<T, F>(run: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| error.to_string())?
}

fn metadata(candidates: &[&Candidate]) -> String {
    let plugins = candidates
        .iter()
        .map(|candidate| {
            let mut object = IndexMap::new();
            object.insert(
                "name".to_owned(),
                OrderedJson::String(candidate.name.clone()),
            );
            object.insert(
                "pluginId".to_owned(),
                OrderedJson::String(candidate.plugin_id.clone()),
            );
            object.insert(
                "directoryName".to_owned(),
                OrderedJson::String(candidate.directory_name.clone()),
            );
            if let Some(enabled) = candidate.enabled_override {
                object.insert("enabled".to_owned(), OrderedJson::Bool(enabled));
            }
            OrderedJson::Object(object)
        })
        .collect();
    let mut root = IndexMap::new();
    root.insert("plugins".to_owned(), OrderedJson::Array(plugins));
    ordered_json::to_pretty(&OrderedJson::Object(root))
        .unwrap_or_else(|_| "{\"plugins\":[]}".to_owned())
}
