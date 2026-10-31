//! 连接管理器的迁移统计与 `source-server-workspace` 管理调用（S4-f，spec §11.6）。

use std::sync::Arc;

use anyhow::{Context, Result};
use escode_host_core::rpc_serialization::WireValue;
use serde_json::{Value, json};

use super::SourceServerConnections;
use crate::source_server_migration_counts::{
    ActiveMigrationConnection, MigrationConnectionLookup, MigrationConnectionToken,
    SOURCE_SERVER_WORKSPACE_CHANNEL,
};

impl SourceServerConnections {
    /// `getSnapshot`：先读目录与连接状态，再合并迁移统计投影；不等待远端 RPC。
    pub fn snapshot(self: &Arc<Self>, refresh_migration_counts: bool) -> Result<Value, String> {
        if refresh_migration_counts {
            self.migration_counts.invalidate(None);
        }
        let lookup = self.migration_connection_lookup();
        loop {
            let start_generation = self.registry.generation();
            let mut snapshot = self.registry.snapshot()?;
            let source_server_ids = source_server_ids(&snapshot);
            let counts = self.migration_counts.read(&source_server_ids, &lookup);
            if self.registry.generation() != start_generation {
                continue;
            }
            snapshot["failedPathCount"] = Value::from(counts.failed_path_count);
            snapshot["abandonedPathCount"] = Value::from(counts.abandoned_path_count);
            snapshot["migrationCounts"] = Value::Array(counts.migration_counts);
            return Ok(snapshot);
        }
    }

    /// 当前 Source 的首选在线物理连接：优先 `preferredProfileId`，否则按 registry profile 顺序。
    pub fn active_connection_for_source(
        &self,
        source_server_id: &str,
    ) -> Option<ActiveMigrationConnection> {
        let preferred = self
            .registry
            .source_server(source_server_id)
            .and_then(|server| server.preferred_profile_id);
        let profile_ids = self.registry.profile_ids_for_source(source_server_id);
        let mut ordered = Vec::with_capacity(profile_ids.len());
        if let Some(preferred) = preferred.filter(|id| profile_ids.contains(id)) {
            ordered.push(preferred);
        }
        let remaining: Vec<String> = profile_ids
            .into_iter()
            .filter(|id| !ordered.contains(id))
            .collect();
        ordered.extend(remaining);
        let slots = self.lock();
        for profile_id in ordered {
            let Some(slot) = slots.get(&profile_id) else {
                continue;
            };
            let Some(connection) = slot.connection.as_ref() else {
                continue;
            };
            return Some(ActiveMigrationConnection {
                token: MigrationConnectionToken {
                    profile_id,
                    attempt: slot.attempt,
                },
                client: Arc::clone(&connection.client),
            });
        }
        None
    }

    fn migration_connection_lookup(self: &Arc<Self>) -> MigrationConnectionLookup {
        // Weak：统计任务不得延长连接管理器生命周期；dispose 后 lookup 直接返回 None。
        let connections = Arc::downgrade(self);
        Arc::new(move |source_server_id: &str| {
            connections
                .upgrade()
                .and_then(|connections| connections.active_connection_for_source(source_server_id))
        })
    }

    /// 迁移重试/放弃后使该 Source 的统计失效；下一次快照重新进入 loading。
    pub fn invalidate_migration_counts(&self, source_server_id: &str) {
        self.migration_counts.invalidate(Some(source_server_id));
    }

    /// 显式读取迁移报告（owner 是远端 Source Server）。
    pub async fn migration_report(&self, source_server_id: &str) -> Result<Value> {
        self.call_workspace(source_server_id, "listMigrationReports", Vec::new())
            .await
    }

    /// 放弃一次失败迁移；统计失效由 RPC 调用方在 finally 语义下完成。
    pub async fn abandon_workspace_migration(
        &self,
        source_server_id: &str,
        workspace_path: &str,
        migration_id: &str,
    ) -> Result<Value> {
        self.call_workspace(
            source_server_id,
            "abandonFailedWorkspace",
            vec![json!({
                "workspacePath": workspace_path,
                "migrationId": migration_id,
            })],
        )
        .await
    }

    /// 重试一次迁移；统计失效由 RPC 调用方在 finally 语义下完成。
    pub async fn retry_workspace_migration(
        &self,
        source_server_id: &str,
        workspace_path: &str,
        migration_id: &str,
    ) -> Result<Value> {
        self.call_workspace(
            source_server_id,
            "retryWorkspaceMigration",
            vec![json!({
                "workspacePath": workspace_path,
                "migrationId": migration_id,
            })],
        )
        .await
    }

    async fn call_workspace(
        &self,
        source_server_id: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value> {
        let connection = self
            .active_connection_for_source(source_server_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Source Server must be connected: {source_server_id}")
            })?;
        let arg = Some(WireValue::Array(
            args.iter().map(WireValue::from_json).collect(),
        ));
        let value = connection
            .client
            .call(SOURCE_SERVER_WORKSPACE_CHANNEL, method, arg)
            .await
            .with_context(|| format!("source-server-workspace.{method} failed"))?;
        value.to_json().with_context(|| {
            format!("source-server-workspace.{method} returned an unsupported value")
        })
    }
}

impl Drop for SourceServerConnections {
    fn drop(&mut self) {
        self.migration_counts.dispose();
    }
}

fn source_server_ids(snapshot: &Value) -> Vec<String> {
    snapshot
        .get("servers")
        .and_then(Value::as_array)
        .map(|servers| {
            servers
                .iter()
                .filter_map(|server| server["sourceServerId"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
