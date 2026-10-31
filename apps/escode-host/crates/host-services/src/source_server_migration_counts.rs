//! Source Server 迁移统计投影（S4-f，docs/specs/source-server-remote-rust-host.md §11.6）。
//!
//! 统计不是持久化事实：owner 只活在 Host 进程内，按当前物理连接 token `(profileId, attempt)`
//! 读取远端 `source-server-workspace`。`getSnapshot` 只读投影，读取由异步任务补全；迟到结果
//! 在连接换代、Source 移除或 owner dispose 后丢弃，不发布 generation。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use escode_host_core::rpc_serialization::WireValue;
use serde_json::{Value, json};

use crate::source_server::SourceServerRegistry;
use crate::source_server_client::RemoteChannelClient;

pub const SOURCE_SERVER_WORKSPACE_CHANNEL: &str = "source-server-workspace";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationConnectionToken {
    pub profile_id: String,
    pub attempt: u64,
}

#[derive(Clone)]
pub struct ActiveMigrationConnection {
    pub token: MigrationConnectionToken,
    pub client: Arc<RemoteChannelClient>,
}

/// 当前物理连接查找器：读取时与迟到结果发布前都用同一份事实判序。
pub type MigrationConnectionLookup =
    Arc<dyn Fn(&str) -> Option<ActiveMigrationConnection> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum EntryValue {
    Loading,
    Available {
        failed_path_count: u64,
        abandoned_path_count: u64,
    },
    Unavailable,
}

struct Entry {
    token: MigrationConnectionToken,
    client: Arc<RemoteChannelClient>,
    value: EntryValue,
    refresh_requested: bool,
}

#[derive(Default)]
struct CountsState {
    entries: BTreeMap<String, Entry>,
    disposed: bool,
}

pub struct MigrationCountsProjection {
    pub failed_path_count: u64,
    pub abandoned_path_count: u64,
    pub migration_counts: Vec<Value>,
}

pub struct SourceServerMigrationCounts {
    state: Mutex<CountsState>,
    registry: Arc<SourceServerRegistry>,
}

impl SourceServerMigrationCounts {
    pub fn new(registry: Arc<SourceServerRegistry>) -> Self {
        Self {
            state: Mutex::new(CountsState::default()),
            registry,
        }
    }

    fn lock(&self) -> MutexGuard<'_, CountsState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 使投影失效：在途只标记一次重读，已完成项直接移除（下次读取重新进入 loading）。
    pub fn invalidate(&self, source_server_id: Option<&str>) {
        let mut state = self.lock();
        let ids: Vec<String> = state
            .entries
            .keys()
            .filter(|id| source_server_id.is_none_or(|target| target == *id))
            .cloned()
            .collect();
        for id in ids {
            let Some(entry) = state.entries.get_mut(&id) else {
                continue;
            };
            if entry.value == EntryValue::Loading {
                entry.refresh_requested = true;
            } else {
                state.entries.remove(&id);
            }
        }
    }

    pub fn dispose(&self) {
        let mut state = self.lock();
        state.disposed = true;
        state.entries.clear();
    }

    /// 同步读取投影：返回当前状态并异步补全 loading 项。调用方不得在此等待远端 RPC。
    pub fn read(
        self: &Arc<Self>,
        source_server_ids: &[String],
        lookup: &MigrationConnectionLookup,
    ) -> MigrationCountsProjection {
        let mut reads = Vec::new();
        let mut migration_counts = Vec::with_capacity(source_server_ids.len());
        {
            let mut state = self.lock();
            if state.disposed {
                return projection_from_entries(Vec::new());
            }
            state
                .entries
                .retain(|id, _| source_server_ids.iter().any(|candidate| candidate == id));
            for source_server_id in source_server_ids {
                let Some(connection) = lookup(source_server_id) else {
                    state.entries.remove(source_server_id);
                    migration_counts.push(json!({
                        "sourceServerId": source_server_id,
                        "status": "requires-connection",
                    }));
                    continue;
                };
                let needs_read = !state
                    .entries
                    .get(source_server_id)
                    .is_some_and(|entry| entry.token == connection.token);
                if needs_read {
                    state.entries.insert(
                        source_server_id.clone(),
                        Entry {
                            token: connection.token.clone(),
                            client: Arc::clone(&connection.client),
                            value: EntryValue::Loading,
                            refresh_requested: false,
                        },
                    );
                    reads.push((
                        source_server_id.clone(),
                        connection.token,
                        connection.client,
                    ));
                }
                let entry = state
                    .entries
                    .get(source_server_id)
                    .expect("entry inserted above");
                migration_counts.push(entry_json(source_server_id, &entry.value));
            }
        }
        let projection = projection_from_entries(migration_counts);
        for (source_server_id, token, client) in reads {
            self.start_read(source_server_id, token, Arc::clone(lookup), client);
        }
        projection
    }

    fn start_read(
        self: &Arc<Self>,
        source_server_id: String,
        token: MigrationConnectionToken,
        lookup: MigrationConnectionLookup,
        client: Arc<RemoteChannelClient>,
    ) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // 同步测试环境没有 runtime：保留 loading，下一次在 runtime 内读取时重新发起。
            return;
        };
        let counts = Arc::clone(self);
        handle.spawn(async move {
            let value = read_remote_counts(&client).await;
            counts.finish_read(source_server_id, token, lookup, value);
        });
    }

    fn finish_read(
        self: &Arc<Self>,
        source_server_id: String,
        token: MigrationConnectionToken,
        lookup: MigrationConnectionLookup,
        value: EntryValue,
    ) {
        let reread = {
            let mut state = self.lock();
            if state.disposed {
                return;
            }
            let Some(entry) = state.entries.get(&source_server_id) else {
                return;
            };
            if entry.token != token {
                return;
            }
            // 结果只属于读取时的物理连接；断线/换代/删除后不得发布。
            let current = lookup(&source_server_id).map(|connection| connection.token);
            if current.as_ref() != Some(&token) {
                state.entries.remove(&source_server_id);
                return;
            }
            let Some(entry) = state.entries.get_mut(&source_server_id) else {
                return;
            };
            if entry.refresh_requested {
                entry.refresh_requested = false;
                Some(Arc::clone(&entry.client))
            } else {
                entry.value = value;
                None
            }
        };
        if let Some(client) = reread {
            let lookup = Arc::clone(&lookup);
            self.start_read(source_server_id, token, lookup, client);
            return;
        }
        self.registry.publish_directory_change();
    }
}

fn projection_from_entries(migration_counts: Vec<Value>) -> MigrationCountsProjection {
    let mut failed_path_count = 0;
    let mut abandoned_path_count = 0;
    for count in &migration_counts {
        if count["status"] != "available" {
            continue;
        }
        failed_path_count += count["failedPathCount"].as_u64().unwrap_or(0);
        abandoned_path_count += count["abandonedPathCount"].as_u64().unwrap_or(0);
    }
    MigrationCountsProjection {
        failed_path_count,
        abandoned_path_count,
        migration_counts,
    }
}

fn entry_json(source_server_id: &str, value: &EntryValue) -> Value {
    match value {
        EntryValue::Loading => json!({
            "sourceServerId": source_server_id,
            "status": "loading",
        }),
        EntryValue::Unavailable => json!({
            "sourceServerId": source_server_id,
            "status": "unavailable",
        }),
        EntryValue::Available {
            failed_path_count,
            abandoned_path_count,
        } => json!({
            "sourceServerId": source_server_id,
            "status": "available",
            "failedPathCount": failed_path_count,
            "abandonedPathCount": abandoned_path_count,
        }),
    }
}

async fn read_remote_counts(client: &RemoteChannelClient) -> EntryValue {
    if !client.has_channel(SOURCE_SERVER_WORKSPACE_CHANNEL) {
        return EntryValue::Unavailable;
    }
    let arg = Some(WireValue::Array(Vec::new()));
    let (failed, abandoned) = tokio::join!(
        client.call(
            SOURCE_SERVER_WORKSPACE_CHANNEL,
            "getFailedPathCount",
            arg.clone()
        ),
        client.call(
            SOURCE_SERVER_WORKSPACE_CHANNEL,
            "getAbandonedPathCount",
            arg
        ),
    );
    match (failed, abandoned) {
        (Ok(failed), Ok(abandoned)) => match (count_value(&failed), count_value(&abandoned)) {
            (Some(failed_path_count), Some(abandoned_path_count)) => EntryValue::Available {
                failed_path_count,
                abandoned_path_count,
            },
            _ => EntryValue::Unavailable,
        },
        // 一项失败也要等另一项收口（join 已满足），整组按不可用处理。
        _ => EntryValue::Unavailable,
    }
}

fn count_value(value: &WireValue) -> Option<u64> {
    match value.to_json()? {
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

#[cfg(test)]
#[path = "source_server_migration_counts_tests.rs"]
mod tests;
