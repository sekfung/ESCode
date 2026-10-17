//! `mcp-sync` 服务（TS `createMcpSyncService`）：候选、状态、导出、导入与配置写入。
//!
//! `listWorkspaceMcpServerStatuses` 是运行态查询，由 RPC 层转发 `mcp/list`（见 `domains_mcp_sync.rs`）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::import::import_servers;
use super::paths::{self, ESCODE, Scope};
use super::read::{self, Record};
use crate::skills::Compare;

#[derive(Clone)]
pub struct McpSyncService {
    home: PathBuf,
    compare: Compare,
}

impl McpSyncService {
    pub fn new(home: PathBuf, compare: Compare) -> Self {
        Self { home, compare }
    }

    pub async fn load_mcp_from_user_directory(&self, params: &Value) -> Result<Value, String> {
        let service = self.clone();
        let params = params.clone();
        blocking(move || read::load(&service.home, &params)).await
    }

    pub async fn save_mcp_to_user_directory(&self, payload: &Value) -> Result<(), String> {
        let service = self.clone();
        let payload = payload.clone();
        blocking(move || read::save(&service.home, &payload)).await
    }

    pub async fn list_local_user_mcp_candidates(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let records = read::effective_user_records(&service.home, &service.compare)?;
            Ok(json!({
                "candidates": records.iter().map(candidate_json).collect::<Vec<_>>(),
                "localHomeDir": service.home.to_string_lossy(),
            }))
        })
        .await
    }

    pub async fn list_remote_user_mcp_statuses(&self, params: &Value) -> Result<Value, String> {
        let service = self.clone();
        let names: Vec<String> = params["names"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        blocking(move || {
            let records = read::effective_user_records(&service.home, &service.compare)?;
            let mut existing: BTreeMap<String, &Record> = BTreeMap::new();
            for record in &records {
                existing.entry(name_key(&record.name)).or_insert(record);
            }
            let statuses: Vec<Value> = names
                .iter()
                .map(|name| match existing.get(&name_key(name)) {
                    Some(record) => json!({
                        "name": name,
                        "exists": true,
                        "path": record.path.to_string_lossy(),
                    }),
                    None => json!({"name": name, "exists": false}),
                })
                .collect();
            Ok(json!({
                "statuses": statuses,
                "remoteHomeDir": service.home.to_string_lossy(),
            }))
        })
        .await
    }

    pub async fn export_mcp_servers(&self, params: &Value) -> Result<Value, String> {
        let service = self.clone();
        let ids: Vec<String> = params["serverIds"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        blocking(move || {
            let records = read::effective_user_records(&service.home, &service.compare)?;
            let by_id: BTreeMap<String, &Record> = records
                .iter()
                .map(|record| (candidate_id(record), record))
                .collect();
            let mut servers = Vec::with_capacity(ids.len());
            for id in &ids {
                let Some(record) = by_id.get(id) else {
                    return Err(format!("mcp sync candidate not found: {id}"));
                };
                servers.push(candidate_json(record));
            }
            Ok(json!({
                "servers": servers,
                "localHomeDir": service.home.to_string_lossy(),
            }))
        })
        .await
    }

    pub async fn check_remote_user_mcp_write_access(&self) -> Result<Value, String> {
        let service = self.clone();
        blocking(move || {
            let path = paths::config_path(ESCODE, Scope::User, &service.home, None)?;
            let directory = path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| service.home.clone());
            Ok(crate::remote_sync::check_directory_write_access(&directory))
        })
        .await
    }

    pub async fn import_mcp_servers(
        &self,
        params: &Value,
        overwrite: bool,
    ) -> Result<Value, String> {
        if overwrite {
            return Err("mcp sync overwrite is not supported".to_owned());
        }
        let service = self.clone();
        let params = params.clone();
        blocking(move || import_servers(&service.home, &service.compare, &params)).await
    }
}

fn candidate_json(record: &Record) -> Value {
    json!({
        "id": candidate_id(record),
        "name": record.name,
        "config": record.config,
        "enabled": record.enabled,
        "source": record.source,
        "path": record.path.to_string_lossy(),
    })
}

fn candidate_id(record: &Record) -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        format!(
            "{}:{}:{}",
            record.source,
            record.path.to_string_lossy(),
            record.name
        )
        .as_bytes(),
    );
    format!("{:x}", hasher.finalize())
}

pub(super) fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub(super) async fn blocking<T, F>(run: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
