//! 连接建立阶段（S4-j）：显式安装 → 身份 probe → hello/Initialize → 握手记录。
//!
//! 从 `source_server_connections.rs` 拆出以守住单文件 400 行门禁；编排顺序与状态语义不变。
//! 未确认的身份变化在这里直接返回 `identity-change-pending`，不启动 server、不写 settings。

use std::sync::Arc;

use anyhow::Result;

use super::SourceServerConnections;
use crate::source_server::{
    ConnectionOperationLog, ConnectionProfile, DeclaredWorkspaceInput, HandshakeRecord,
};
use crate::source_server_bootstrap::{apply_install, install_error_code, prepare_install};
use crate::source_server_compatibility::CompatibilityEvaluation;
use crate::source_server_connection::RemoteConnection;
use crate::source_server_connection_open::{OpenFailure, open_connection};
use crate::source_server_connection_state::now_ms;
use crate::source_server_connections::ConnectIntent;
use crate::source_server_transport::is_host_key_changed_error;
use tokio::sync::watch;

pub(super) struct Established {
    pub connection: RemoteConnection,
    pub closed: watch::Receiver<bool>,
    pub version: String,
    pub compatibility: Option<CompatibilityEvaluation>,
    pub effective: ConnectionProfile,
}

impl SourceServerConnections {
    /// 安装 → probe → 打开连接 → 记录握手；返回实际用于后续编排的 effective profile。
    pub(super) async fn establish(
        &self,
        profile: &ConnectionProfile,
        intent: ConnectIntent,
    ) -> Result<Established> {
        let transport = match (self.options.resolve_transport)(profile).await {
            Ok(transport) => transport,
            Err(error) => return Err(self.state().fail(profile, error, None)),
        };
        if matches!(
            intent,
            ConnectIntent::Explicit | ConnectIntent::ConfirmIdentityChange
        ) && let Some(install) = self.options.install.as_ref()
        {
            match prepare_install(transport.runner.as_ref(), &install.assets).await {
                Ok(None) => {}
                Ok(Some(prepared)) => {
                    self.state().publish_install(profile, &prepared.preview);
                    if let Err(error) =
                        apply_install(transport.runner.as_ref(), &prepared, &install.lock).await
                    {
                        let code = install_error_code(&error);
                        return Err(self.state().fail(profile, error, code));
                    }
                    self.registry
                        .append_operation_log(ConnectionOperationLog::new(
                            &profile.profile_id,
                            "connect",
                            "installed",
                            "info",
                            format!(
                                "installed source server {} for {}",
                                prepared.manifest.app_version, prepared.plan.platform_arch
                            ),
                            now_ms(),
                        ));
                }
                Err(error) => {
                    let code = install_error_code(&error);
                    return Err(self.state().fail(profile, error, code));
                }
            }
        }
        let confirm_identity = intent == ConnectIntent::ConfirmIdentityChange;
        let probed = match self.options.identity_probe.as_ref() {
            Some(probe) => match (probe)(Arc::clone(&transport.runner)).await {
                Ok(identity) => Some(identity),
                Err(error) => return Err(self.fail_probe(profile, error)),
            },
            None => None,
        };
        if let Some(identity) = &probed
            && !confirm_identity
            && profile
                .source_server_id
                .as_deref()
                .is_some_and(|previous| previous != identity.source_server_id)
        {
            return Err(self
                .state()
                .identity_change_pending(profile, &identity.source_server_id));
        }
        let opened = match open_connection(&self.options, &transport.command).await {
            Ok(opened) => opened,
            Err(OpenFailure::Incompatible {
                evaluation,
                version,
            }) => {
                return Err(self
                    .state()
                    .fail_incompatible(profile, &evaluation, &version));
            }
            Err(OpenFailure::Timeout(error)) => {
                return Err(self
                    .state()
                    .fail(profile, error, Some("remote-connection-timeout")));
            }
            Err(OpenFailure::HostKeyChanged(error)) => {
                return Err(self.state().fail_host_key_changed(profile, error));
            }
            Err(OpenFailure::Other(error)) => {
                return Err(self.state().fail(profile, error, None));
            }
        };
        let version = opened.version;
        let compatibility = opened.compatibility;
        let connection = opened.connection;
        let closed = connection.closed();
        let Some(identity) = probed else {
            return Ok(Established {
                connection,
                closed,
                version,
                compatibility,
                effective: profile.clone(),
            });
        };
        let record = HandshakeRecord {
            source_server_id: identity.source_server_id.clone(),
            os_user: identity.os_user,
            hostname: identity.hostname,
            version: version.clone(),
            connected_at: now_ms(),
            workspaces: identity
                .workspace_path
                .into_iter()
                .map(|path| DeclaredWorkspaceInput {
                    path,
                    workspace_identity: None,
                    label: None,
                })
                .collect(),
        };
        let outcome = match self
            .registry
            .record_handshake(
                &profile.profile_id,
                &record,
                confirm_identity,
                self.options.credentials.as_deref(),
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = connection.shutdown().await;
                return Err(self.state().fail(profile, error, None));
            }
        };
        if outcome.identity_changed && !confirm_identity {
            let _ = connection.shutdown().await;
            return Err(self
                .state()
                .identity_change_pending(profile, &record.source_server_id));
        }
        Ok(Established {
            connection,
            closed,
            version,
            compatibility,
            effective: outcome.effective_profile,
        })
    }

    /// probe 失败也要沿用 §11.8 的错误归因：host key 变化不可重试，超时可重试。
    fn fail_probe(&self, profile: &ConnectionProfile, error: anyhow::Error) -> anyhow::Error {
        let message = format!("{error:#}");
        if is_host_key_changed_error(&message) {
            return self.state().fail_host_key_changed(profile, error);
        }
        if message.contains("remote command timed out") {
            return self
                .state()
                .fail(profile, error, Some("remote-connection-timeout"));
        }
        self.state().fail(profile, error, None)
    }
}
