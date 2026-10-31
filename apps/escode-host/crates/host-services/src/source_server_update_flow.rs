//! 显式更新编排（S4-d）：target 校验 → 运行任务 guard → 事务安装 → 连接换代/回滚。
//!
//! 本模块只编排 `SourceServerConnections` 暴露的连接原语；连接状态仍写入 registry，
//! 事件与快照保持单一来源。

use std::sync::Arc;

use serde_json::Value;

use crate::source_server::{
    ConnectionOperationLog, ConnectionProfile, ConnectionStateSnapshot, DeclaredWorkspace,
    RunningTask, SourceServerRegistry,
};
use crate::source_server_client::RemoteChannelClient;
use crate::source_server_connection_open::{open_connection, open_raw_connection};
use crate::source_server_connection_projection::online_state;
use crate::source_server_connection_state::now_ms;
use crate::source_server_connections::SourceServerConnections;
use crate::source_server_exec::ResolvedTransport;
use crate::source_server_provider_provisioning::{
    ProviderProvisioningPeer, ProvisioningTrigger, RemoteProvisioningPeer,
};
use crate::source_server_transport::TransportCommand;
use crate::source_server_update::{
    TASK_CHANNEL, TaskInspection, UpdateError, UpdateRequest, declared_workspaces,
    expected_target_matches, parse_running_tasks, prepare_update, task_list_request,
    update_failure_state, updating_state,
};
use crate::source_server_update_install::UpdateInstallTransaction;
use escode_host_core::rpc_serialization::WireValue;

pub(crate) async fn update_source(
    connections: &SourceServerConnections,
    request: UpdateRequest,
) -> std::result::Result<(), UpdateError> {
    let registry = connections.registry().clone();
    let profile = registry.profile(&request.profile_id).ok_or_else(|| {
        UpdateError::new(
            None,
            Some(false),
            format!("unknown source server profile: {}", request.profile_id),
        )
    })?;
    if let Some(expected) = &request.expected_target {
        match expected_target_matches(&profile, expected) {
            Ok(true) => {}
            Ok(false) => {
                return Err(UpdateError::new(
                    Some("remote-target-changed"),
                    Some(false),
                    "connection target changed; retry the connection",
                ));
            }
            Err(error) => {
                return Err(UpdateError::new(
                    Some("remote-target-changed"),
                    Some(false),
                    format!("invalid expected target: {error:#}"),
                ));
            }
        }
    }
    let previous = registry
        .connection_state(&profile.profile_id)
        .ok_or_else(|| UpdateError::new(None, Some(false), "profile state is unavailable"))?;
    let starting = connections.starting_lock(&profile.profile_id);
    let _guard = starting.lock_owned().await;

    let Some(install) = connections.options().install.as_ref() else {
        let error = UpdateError::new(
            Some("remote-install-failed"),
            Some(true),
            "source server update needs remote assets from Main",
        );
        publish_update_failure(&registry, &previous, &error, None);
        return Err(error);
    };
    let transport = match (connections.options().resolve_transport)(&profile).await {
        Ok(transport) => transport,
        Err(cause) => {
            let error = UpdateError::new(
                crate::source_server_bootstrap::install_error_code(&cause),
                Some(true),
                format!("{cause:#}"),
            );
            publish_update_failure(&registry, &previous, &error, None);
            return Err(error);
        }
    };
    // server 目标的 runtime 由服务端自己持有，Host 不执行 local-upload / remote-download 更新。
    let Some(command) = transport.link.process() else {
        let error = UpdateError::new(
            Some("source-server-update-unsupported"),
            Some(false),
            "server transport has no desktop-managed update",
        );
        publish_update_failure(&registry, &previous, &error, None);
        return Err(error);
    };
    let prepared = match prepare_update(transport.runner.as_ref(), &install.assets).await {
        Ok(prepared) => prepared,
        Err(cause) => {
            let error = UpdateError::new(
                crate::source_server_bootstrap::install_error_code(&cause)
                    .or(Some("remote-install-failed")),
                Some(true),
                format!("{cause:#}"),
            );
            publish_update_failure(&registry, &previous, &error, None);
            return Err(error);
        }
    };

    let source_server = profile
        .source_server_id
        .as_deref()
        .and_then(|source_server_id| registry.source_server(source_server_id));
    let declared = declared_workspaces(source_server.as_ref());
    let mut running_tasks = None;
    if !request.force {
        match inspect_running_tasks(connections, &profile, &transport, &prepared, &declared).await {
            TaskInspection::Known(tasks) if !tasks.is_empty() => {
                let error = UpdateError::new(
                    Some("source-server-update-running-tasks"),
                    Some(false),
                    format!(
                        "source server update blocked by {} running task(s)",
                        tasks.len()
                    ),
                );
                publish_update_failure(&registry, &previous, &error, Some(tasks));
                return Err(error);
            }
            TaskInspection::Known(tasks) => running_tasks = Some(tasks),
            TaskInspection::Unavailable(reason) => {
                // 清单不可读时不得把「未知」当成「没有任务」（spec §11.4）。
                let error = UpdateError::new(
                    Some("source-server-update-running-tasks"),
                    Some(false),
                    format!(
                        "source server running tasks are unavailable: {reason}; force update is required"
                    ),
                );
                publish_update_failure(&registry, &previous, &error, None);
                return Err(error);
            }
        }
    }

    registry.set_connection_state(updating_state(
        &profile,
        &previous,
        &prepared.install.preview,
        running_tasks.clone(),
        now_ms(),
    ));
    registry.append_operation_log(ConnectionOperationLog::new(
        &profile.profile_id,
        "update",
        "installing",
        "info",
        format!(
            "updating source server to {}",
            prepared.install.manifest.app_version
        ),
        now_ms(),
    ));

    let transaction = match UpdateInstallTransaction::apply(
        Arc::clone(&transport.runner),
        &prepared.install,
        &install.lock,
    )
    .await
    {
        Ok(transaction) => transaction,
        Err(cause) => {
            let error = UpdateError::new(
                crate::source_server_bootstrap::install_error_code(&cause)
                    .or(Some("remote-install-failed")),
                Some(true),
                format!("{cause:#}"),
            );
            publish_update_failure(&registry, &previous, &error, running_tasks);
            return Err(error);
        }
    };

    // 先让旧连接失效再换代：watcher 的 offline 属于过期尝试，不能覆盖更新结果。
    let (attempt, previous_connection) = connections.begin_update_swap(&profile.profile_id);
    let had_previous_connection = previous_connection.is_some();
    if let Some(connection) = previous_connection {
        let _ = connection.shutdown().await;
    }

    match open_connection(connections.options(), &transport.link, None).await {
        Ok(opened) => {
            let crate::source_server_connection_open::OpenedConnection {
                connection,
                version,
                compatibility,
            } = opened;
            let closed = connection.closed();
            let client = Arc::clone(&connection.client);
            if !connections.store_if_current(&profile.profile_id, attempt, connection) {
                let _ = transaction.rollback().await;
                if had_previous_connection {
                    restore_connection_after_rollback(connections, &profile, attempt, command)
                        .await;
                }
                let error = UpdateError::new(
                    Some("remote-connection-canceled"),
                    Some(true),
                    "source server update was superseded",
                );
                publish_update_failure(&registry, &previous, &error, running_tasks);
                return Err(error);
            }
            // 更新换代是既有 Environment 的重连：整份重下发，但失败只 warning，不回滚更新。
            if let Some(provisioning) = connections.options().provisioning.clone() {
                let peer: Arc<dyn ProviderProvisioningPeer> =
                    Arc::new(RemoteProvisioningPeer::new(client));
                let outcome = provisioning
                    .sync(
                        &profile.profile_id,
                        attempt,
                        peer,
                        ProvisioningTrigger::EnvironmentOnline,
                    )
                    .await;
                if !outcome.allows_online()
                    && connections.current_attempt(&profile.profile_id) == attempt
                {
                    provisioning.log_warning(
                        &profile.profile_id,
                        ProvisioningTrigger::EnvironmentOnline,
                        &outcome,
                    );
                }
            }
            transaction.commit().await;
            connections.record_connected();
            let state = online_state(
                &profile,
                &version,
                compatibility.as_ref(),
                &connections.options().compatibility,
                now_ms(),
            );
            registry.set_connection_state(state);
            registry.append_operation_log(ConnectionOperationLog::new(
                &profile.profile_id,
                "update",
                "updated",
                "info",
                format!("source server updated to {version}"),
                now_ms(),
            ));
            connections.watch_close(profile.profile_id.clone(), attempt, closed);
            Ok(())
        }
        Err(failure) => {
            let (message, code) = failure.message();
            let rollback = transaction.rollback().await;
            let restored = if had_previous_connection {
                restore_connection_after_rollback(connections, &profile, attempt, command).await
            } else {
                false
            };
            let mut error = UpdateError::new(code, Some(true), message);
            if let Err(rollback) = rollback {
                error.message = format!(
                    "{}; restore previous release failed: {rollback:#}",
                    error.message
                );
            }
            if had_previous_connection && !restored {
                error.message = format!(
                    "{}; previous connection could not be restored",
                    error.message
                );
                let mut state = update_failure_state(
                    &previous,
                    error.message.clone(),
                    error.code,
                    error.retryable,
                    running_tasks,
                    now_ms(),
                );
                state.state = "offline";
                registry.set_connection_state(state);
            } else {
                publish_update_failure(&registry, &previous, &error, running_tasks);
            }
            Err(error)
        }
    }
}

fn publish_update_failure(
    registry: &SourceServerRegistry,
    previous: &ConnectionStateSnapshot,
    error: &UpdateError,
    running_tasks: Option<Vec<RunningTask>>,
) {
    registry.set_connection_state(update_failure_state(
        previous,
        error.message.clone(),
        error.code,
        error.retryable,
        running_tasks,
        now_ms(),
    ));
    registry.append_operation_log(ConnectionOperationLog::new(
        &previous.profile_id,
        "update",
        "failed",
        "error",
        error.message.clone(),
        now_ms(),
    ));
}

async fn inspect_running_tasks(
    connections: &SourceServerConnections,
    profile: &ConnectionProfile,
    transport: &ResolvedTransport,
    prepared: &crate::source_server_update::PreparedUpdate,
    declared: &[DeclaredWorkspace],
) -> TaskInspection {
    if declared.is_empty() {
        return TaskInspection::Known(Vec::new());
    }
    let request = task_list_request(declared);
    if let Some(client) = connections.client(&profile.profile_id) {
        return inspect_task_client(&client, request).await;
    }
    if !prepared.installed {
        // 远端没有 server 可运行，运行任务必然为空；首装不需要额外连接。
        return TaskInspection::Known(Vec::new());
    }
    let Some(command) = transport.link.process() else {
        // server 链路不参与 Host 管理的更新，这里只是防御性收口。
        return TaskInspection::Unavailable("server transport has no remote script".to_owned());
    };
    match open_raw_connection(connections.options(), command).await {
        Ok(connection) => {
            let client = Arc::clone(&connection.client);
            let inspection = inspect_task_client(&client, request).await;
            let _ = connection.shutdown().await;
            inspection
        }
        Err(error) => TaskInspection::Unavailable(format!("{error:#}")),
    }
}

async fn restore_connection_after_rollback(
    connections: &SourceServerConnections,
    profile: &ConnectionProfile,
    attempt: u64,
    command: &TransportCommand,
) -> bool {
    match open_raw_connection(connections.options(), command).await {
        Ok(connection) => {
            let closed = connection.closed();
            if !connections.store_if_current(&profile.profile_id, attempt, connection) {
                return false;
            }
            connections.watch_close(profile.profile_id.clone(), attempt, closed);
            true
        }
        Err(error) => {
            escode_host_control::logging::warn(
                "source-server",
                format!(
                    "restore previous connection {}: {error:#}",
                    profile.profile_id
                ),
            );
            false
        }
    }
}

async fn inspect_task_client(client: &RemoteChannelClient, request: Value) -> TaskInspection {
    if !client.has_channel(TASK_CHANNEL) {
        return TaskInspection::Unavailable(format!(
            "remote server does not expose {TASK_CHANNEL}"
        ));
    }
    match client
        .call(
            TASK_CHANNEL,
            "listTaskList",
            Some(WireValue::from_json(&request)),
        )
        .await
    {
        Ok(value) => match value.to_json() {
            Some(value) => match parse_running_tasks(&value) {
                Ok(tasks) => TaskInspection::Known(tasks),
                Err(error) => TaskInspection::Unavailable(format!("{error:#}")),
            },
            None => TaskInspection::Unavailable("listTaskList returned no JSON value".to_owned()),
        },
        Err(error) => TaskInspection::Unavailable(format!("listTaskList failed: {error}")),
    }
}
