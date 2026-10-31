//! 建立远端 server 连接（S3/S4-d）：链路启动 → 握手/Initialize → 兼容判定。
//!
//! `connect_initialized` 只做握手与 Initialize；连接编排（状态发布、尝试代次）留在
//! `source_server_connections`，更新探针也复用同一路径。
//!
//! 链路差异只有两种（S5-f-3）：子进程 stdio 需要 `escode-hello` + ack；server 直连
//! 复用调用方已取得的 `/api/server-info` 探测结果升级 WebSocket，没有 hello 步骤。

use anyhow::Result;

use crate::source_server_compatibility::{CompatibilityEvaluation, evaluate};
use crate::source_server_connection::{ConnectOptions, RemoteConnection};
use crate::source_server_connections::ConnectionsOptions;
use crate::source_server_exec::RemoteLink;
use crate::source_server_transport::{TransportCommand, is_host_key_changed_error};
use crate::source_server_websocket::{self, ServerProbe};

/// 已握手且 Initialize 完成的连接，以及兼容判定结果。
pub(crate) struct OpenedConnection {
    pub connection: RemoteConnection,
    pub version: String,
    pub compatibility: Option<CompatibilityEvaluation>,
}

pub(crate) enum OpenFailure {
    Incompatible {
        evaluation: CompatibilityEvaluation,
        version: String,
    },
    Timeout(anyhow::Error),
    HostKeyChanged(anyhow::Error),
    Other(anyhow::Error),
}

impl OpenFailure {
    pub(crate) fn message(&self) -> (String, Option<&'static str>) {
        match self {
            Self::Incompatible {
                evaluation,
                version,
            } => (
                format!(
                    "remote source server {version} is incompatible: requires {}",
                    evaluation.required_minimum_version
                ),
                Some("source-server-upgrade-required"),
            ),
            Self::Timeout(error) => (format!("{error:#}"), Some("remote-connection-timeout")),
            Self::HostKeyChanged(error) => (format!("{error:#}"), Some("remote-host-key-changed")),
            Self::Other(error) => (format!("{error:#}"), None),
        }
    }
}

pub(crate) async fn connect_initialized(
    options: &ConnectionsOptions,
    link: &RemoteLink,
    server_probe: Option<ServerProbe>,
) -> std::result::Result<RemoteConnection, OpenFailure> {
    let connect_options = ConnectOptions {
        client_id: &options.client_id,
        client_version: &options.client_version,
        handshake_timeout: options.initialize_timeout,
    };
    let connection = match link {
        RemoteLink::Process(command) => {
            crate::source_server_connection::connect(command, &connect_options)
                .await
                .map_err(|error| {
                    if is_host_key_changed_error(&format!("{error:#}")) {
                        OpenFailure::HostKeyChanged(error)
                    } else {
                        OpenFailure::Other(error)
                    }
                })?
        }
        RemoteLink::Server(connect) => {
            let Some(probe) = server_probe else {
                return Err(OpenFailure::Other(anyhow::anyhow!(
                    "server link requires an identity probe before opening"
                )));
            };
            tokio::time::timeout(
                options.initialize_timeout,
                source_server_websocket::open(&connect.url, connect.token.as_deref(), probe),
            )
            .await
            .map_err(|_| {
                OpenFailure::Timeout(anyhow::anyhow!(
                    "websocket handshake timeout: no /ws/host within {:?}",
                    options.initialize_timeout
                ))
            })?
            .map_err(OpenFailure::Other)?
        }
    };
    match tokio::time::timeout(
        options.initialize_timeout,
        connection.client.wait_initialized(),
    )
    .await
    {
        Ok(Ok(())) => Ok(connection),
        Ok(Err(error)) => {
            let _ = connection.shutdown().await;
            Err(OpenFailure::Other(error))
        }
        Err(_) => {
            let _ = connection.shutdown().await;
            Err(OpenFailure::Timeout(anyhow::anyhow!(
                "initialize timeout: no Initialize within {:?}",
                options.initialize_timeout
            )))
        }
    }
}

pub(crate) async fn open_raw_connection(
    options: &ConnectionsOptions,
    command: &TransportCommand,
) -> Result<RemoteConnection> {
    connect_initialized(options, &RemoteLink::Process(command.clone()), None)
        .await
        .map_err(|failure| match failure {
            OpenFailure::Timeout(error)
            | OpenFailure::HostKeyChanged(error)
            | OpenFailure::Other(error) => error,
            OpenFailure::Incompatible {
                evaluation,
                version,
            } => anyhow::anyhow!(
                "remote source server {version} is incompatible: {}",
                evaluation.required_minimum_version
            ),
        })
}

pub(crate) async fn open_connection(
    options: &ConnectionsOptions,
    link: &RemoteLink,
    server_probe: Option<ServerProbe>,
) -> std::result::Result<OpenedConnection, OpenFailure> {
    let connection = connect_initialized(options, link, server_probe).await?;
    let version = connection.info.version.clone();
    let compatibility = match connection.info.compatibility.as_ref() {
        None => None,
        Some(remote) => match evaluate(&options.compatibility, remote) {
            Ok(evaluation) => Some(evaluation),
            Err(error) => {
                let _ = connection.shutdown().await;
                return Err(OpenFailure::Other(error));
            }
        },
    };
    if let Some(evaluation) = &compatibility
        && !evaluation.compatible
    {
        let evaluation = evaluation.clone();
        let _ = connection.shutdown().await;
        return Err(OpenFailure::Incompatible {
            evaluation,
            version,
        });
    }
    Ok(OpenedConnection {
        connection,
        version,
        compatibility,
    })
}
