//! Source Server 显式更新（S4-d，docs/specs/source-server-remote-rust-host.md §11.4）。
//!
//! 更新不是连接流程的一部分：`updateSource` 先校验不可变执行目标，再按 Source 声明的
//! workspace 读取远端运行任务；`force:false` 有运行任务（或清单不可用）时只返回 guard 错误。
//! 安装阶段先备份旧 release、写 pending marker，再替换组件；新 server 握手成功后才提交，
//! 失败则回滚 release 并恢复旧连接。

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::source_server::{
    ConnectionProfile, ConnectionStateSnapshot, DeclaredWorkspace, RunningTask, SourceServerRecord,
};
use crate::source_server_bootstrap::{
    PreparedInstall, RemoteAssetsConfig, build_preview, choose_mode, probe_remote, resolve_manifest,
};
use crate::source_server_exec::RemoteRunner;
use crate::source_server_install::build_install_plan;
use crate::source_server_release::InstallPreview;
use crate::source_server_transport::{TransportTarget, parse_target};

/// 任务清单 owner 的 channel（spec §11.4 明确使用 `listTaskList`，不读远端私有文件）。
pub const TASK_CHANNEL: &str = "escode-task";

/// 更新失败：错误码与 retryable 由 Host 决定，RPC 层直接透传（spec §11.4）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateError {
    pub message: String,
    pub code: Option<&'static str>,
    pub retryable: Option<bool>,
}

impl UpdateError {
    pub fn new(
        code: Option<&'static str>,
        retryable: Option<bool>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            code,
            retryable,
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UpdateError {}

/// `updateSource` 的严格入参（TS `sourceServerUpdateRequestSchema`）。
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateRequest {
    pub profile_id: String,
    pub force: bool,
    pub expected_target: Option<Value>,
}

impl UpdateRequest {
    pub fn from_json(value: &Value) -> Result<Self> {
        let map = value
            .as_object()
            .ok_or_else(|| anyhow!("updateSource requires an object argument"))?;
        reject_unknown(map, &["profileId", "force", "expectedTarget"])?;
        let profile_id = required_text(map, "profileId")?;
        let force = match map.get("force") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => bail!("updateSource.force must be boolean"),
        };
        let expected_target = match map.get("expectedTarget") {
            None | Some(Value::Null) => None,
            Some(value @ Value::Object(_)) => Some(value.clone()),
            Some(_) => bail!("updateSource.expectedTarget must be an object"),
        };
        Ok(Self {
            profile_id,
            force,
            expected_target,
        })
    }
}

/// 不可变执行目标（只含身份字段，不含凭据）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateTargetIdentity {
    Ssh {
        host: String,
        port: u16,
        username: String,
        ssh_config_alias: Option<String>,
    },
    Wsl {
        distro: Option<String>,
        user: Option<String>,
    },
    Docker {
        container: String,
    },
}

pub fn profile_target_identity(profile: &ConnectionProfile) -> Result<UpdateTargetIdentity> {
    let alias = profile
        .target
        .get("sshConfigAlias")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(match parse_target(profile)? {
        TransportTarget::Ssh(ssh) => UpdateTargetIdentity::Ssh {
            host: ssh.host.to_lowercase(),
            port: ssh.port,
            username: ssh.username,
            ssh_config_alias: alias,
        },
        TransportTarget::Wsl(wsl) => UpdateTargetIdentity::Wsl {
            distro: wsl.distro.filter(|value| !value.trim().is_empty()),
            user: wsl.user.filter(|value| !value.trim().is_empty()),
        },
        TransportTarget::Docker(docker) => UpdateTargetIdentity::Docker {
            container: docker.container,
        },
        // server 目标的 runtime 由服务端自己持有，Host 不执行 local-upload / remote-download 更新。
        TransportTarget::Server(_) => bail!("source-server-update-unsupported"),
    })
}

pub fn expected_target_identity(value: &Value) -> Result<UpdateTargetIdentity> {
    let map = value
        .as_object()
        .ok_or_else(|| anyhow!("expectedTarget must be an object"))?;
    let kind = required_text(map, "kind")?;
    match kind.as_str() {
        "ssh" => {
            reject_unknown(map, &["kind", "host", "port", "username", "sshConfigAlias"])?;
            let port = match map.get("port") {
                None | Some(Value::Null) => 22,
                Some(value) => value
                    .as_u64()
                    .and_then(|value| u16::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| anyhow!("expectedTarget.port must be 1..65535"))?,
            };
            Ok(UpdateTargetIdentity::Ssh {
                host: required_text(map, "host")?.to_lowercase(),
                port,
                username: required_text(map, "username")?,
                ssh_config_alias: optional_text(map, "sshConfigAlias"),
            })
        }
        "wsl" => {
            reject_unknown(map, &["kind", "distro", "user"])?;
            Ok(UpdateTargetIdentity::Wsl {
                distro: optional_text(map, "distro"),
                user: optional_text(map, "user"),
            })
        }
        "docker" => {
            reject_unknown(map, &["kind", "container"])?;
            Ok(UpdateTargetIdentity::Docker {
                container: required_text(map, "container")?,
            })
        }
        other => bail!("unsupported expectedTarget.kind: {other}"),
    }
}

/// Profile 与授权目标必须一致；WSL 缺 distro/user 时无法证明身份，按不匹配处理。
pub fn expected_target_matches(profile: &ConnectionProfile, expected: &Value) -> Result<bool> {
    Ok(profile_target_identity(profile)? == expected_target_identity(expected)?)
}

/// 远端更新准备：始终包含 manifest 全量组件（与首连只装缺失组件不同）。
pub struct PreparedUpdate {
    pub install: PreparedInstall,
    /// 必需组件已存在。为 false 时不存在运行中的旧 server，无需临时连接读任务。
    pub installed: bool,
}

pub async fn prepare_update(
    runner: &dyn RemoteRunner,
    assets: &RemoteAssetsConfig,
) -> Result<PreparedUpdate> {
    let probe = probe_remote(runner).await?;
    let platform_arch = probe.environment.platform_arch.clone();
    let manifest = resolve_manifest(assets, &platform_arch)?;
    let plan = build_install_plan(&manifest)?;
    let inspected = crate::source_server_bootstrap::inspect_remote(runner, &plan).await?;
    let installed = ["node-runtime", "server-bundle"]
        .iter()
        .all(|id| inspected.get(*id).is_some_and(|component| component.exists));
    let mode = choose_mode(assets, &plan, &probe)?;
    let preview = build_preview(assets, &manifest, &probe.environment, &plan, &mode);
    Ok(PreparedUpdate {
        install: PreparedInstall {
            manifest,
            plan,
            mode,
            preview,
        },
        installed,
    })
}

/// 运行任务清单读取结果：`Unavailable` 不得被当成空清单（spec §11.4）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskInspection {
    Known(Vec<RunningTask>),
    Unavailable(String),
}

pub fn task_list_request(declared: &[DeclaredWorkspace]) -> Value {
    let scopes: Vec<Value> = declared
        .iter()
        .map(|workspace| {
            let mut scope = json!({
                "workspacePath": workspace.workspace_path,
                "workspaceIdentity": workspace.workspace_identity,
            });
            if let Some(label) = &workspace.label {
                scope["label"] = Value::from(label.clone());
            }
            scope
        })
        .collect();
    json!({
        "kind": "active",
        "workspaceScopes": scopes,
        "limit": Value::Null,
    })
}

pub fn parse_running_tasks(value: &Value) -> Result<Vec<RunningTask>> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("task list response has no items array"))?;
    let mut tasks = Vec::new();
    for item in items {
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .or_else(|| item.get("liveStatus").and_then(Value::as_str));
        let running =
            status == Some("running") || item.get("running").and_then(Value::as_bool) == Some(true);
        if !running {
            continue;
        }
        let session_id = item
            .get("taskId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("running task has no taskId"))?;
        let workspace_path = item
            .get("workspacePath")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("running task {session_id} has no workspacePath"))?;
        tasks.push(RunningTask {
            session_id: session_id.to_owned(),
            workspace_path: workspace_path.to_owned(),
            workspace_identity: item
                .get("workspaceIdentity")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        });
    }
    Ok(tasks)
}

pub fn declared_workspaces(source_server: Option<&SourceServerRecord>) -> Vec<DeclaredWorkspace> {
    source_server
        .map(|server| server.declared_workspaces.clone())
        .unwrap_or_default()
}

/// 更新失败投影：保留操作前状态（online 仍是 online），只附加错误与可重试信息。
pub fn update_failure_state(
    previous: &ConnectionStateSnapshot,
    error: String,
    code: Option<&str>,
    retryable: Option<bool>,
    running_tasks: Option<Vec<RunningTask>>,
    updated_at: u64,
) -> ConnectionStateSnapshot {
    let mut state = previous.clone();
    state.error = Some(error);
    state.error_code = code.map(str::to_owned);
    state.retryable = retryable;
    state.running_tasks = running_tasks;
    state.updated_at = updated_at;
    state
}

/// 更新执行中：覆盖旧的 required/error 文案，只保留当前版本与安装预览。
pub fn updating_state(
    profile: &ConnectionProfile,
    previous: &ConnectionStateSnapshot,
    preview: &Option<InstallPreview>,
    running_tasks: Option<Vec<RunningTask>>,
    updated_at: u64,
) -> ConnectionStateSnapshot {
    let mut state = ConnectionStateSnapshot::new(
        &profile.profile_id,
        profile.source_server_id.as_deref(),
        "updating",
        updated_at,
    );
    state.current_version = previous.current_version.clone();
    state.install_preview = preview.clone();
    state.running_tasks = running_tasks;
    state
}

fn reject_unknown(map: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("unexpected field: {key}");
    }
    Ok(())
}

fn required_text(map: &Map<String, Value>, key: &str) -> Result<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{key} must be a non-empty string"))
}

fn optional_text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "source_server_update_tests.rs"]
mod tests;
