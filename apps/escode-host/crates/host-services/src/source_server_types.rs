//! Source Server registry 数据模型（S2–S4，docs/specs/source-server-remote-rust-host.md §9）。
//!
//! 从 `source_server.rs` 拆出以守住单文件 400 行门禁：schema 默认值、持久化记录、连接状态投影。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::source_server_release::InstallPreview;

fn default_install_state() -> String {
    "installed".to_owned()
}

/// Connection Profile 的 schema 默认值（TS `connectionProfileBaseShape`）。
fn default_lifecycle_state() -> String {
    "active".to_owned()
}

/// 声明过的远端 workspace（`SourceServerRecord.declaredWorkspaces`）。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredWorkspace {
    pub workspace_path: String,
    pub workspace_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// 控制端已识别的 Source Server（TS `sourceServerRecordSchema`）。
///
/// 反序列化忽略未知字段、序列化只输出 schema 允许的键——TS 侧是 `.strict()`，
/// 透传未知键会让快照解析失败（spec §9.2）。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceServerRecord {
    pub source_server_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_online_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_profile_id: Option<String>,
    #[serde(default = "default_install_state")]
    pub install_state: String,
    #[serde(default)]
    pub declared_workspaces: Vec<DeclaredWorkspace>,
}

/// 到达 Source Server 的线路（TS `connectionProfileSchema`）。
///
/// `target` 按 transport 原样透传：它由写入方按 transport 判别式 schema 生成，
/// 与 UI 直接读 settings 的形状一致（spec §9.1）。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub profile_id: String,
    pub transport: String,
    pub target: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connect_error: Option<String>,
    pub created_at: u64,
    #[serde(default = "default_lifecycle_state")]
    pub lifecycle_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<u64>,
}

/// workspace 打开历史（TS `sourceServerWorkspaceHistoryEntrySchema`）。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHistoryEntry {
    pub workspace_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_workspace_identity: Option<String>,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_server_id: Option<String>,
    pub last_opened_at: u64,
}

/// 远端任务 owner 的活动投影（TS `sourceServerRunningTaskSchema`）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunningTask {
    pub session_id: String,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
}

/// 连接状态投影（TS `sourceServerConnectionStateSnapshotSchema` 的可读子集）。
///
/// 字段是 TS schema 的子集：S2 从 settings 派生 `absent` / `failed`（spec §9.3）；
/// S3 由连接管理器（spec §10.10）写入运行态状态，并补 `errorCode` / `retryable` /
/// `currentVersion`。未知字段不输出，避免严格 schema 解析失败。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStateSnapshot {
    pub profile_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_server_id: Option<String>,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_availability: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade_target: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_preview: Option<InstallPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_tasks: Option<Vec<RunningTask>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_source_server_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_budget_exhausted: Option<bool>,
    pub updated_at: u64,
}

impl ConnectionStateSnapshot {
    pub fn new(
        profile_id: &str,
        source_server_id: Option<&str>,
        state: &'static str,
        updated_at: u64,
    ) -> Self {
        Self {
            profile_id: profile_id.to_owned(),
            source_server_id: source_server_id.map(str::to_owned),
            state,
            error: None,
            error_code: None,
            retryable: None,
            current_version: None,
            update_availability: None,
            available_version: None,
            minimum_version: None,
            upgrade_target: None,
            install_preview: None,
            running_tasks: None,
            pending_source_server_id: None,
            recovery_attempt: None,
            recovery_limit: None,
            recovery_budget_exhausted: None,
            updated_at,
        }
    }
}
