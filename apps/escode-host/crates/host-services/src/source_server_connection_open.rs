//! 建立远端 server 连接（S3/S4-d）：transport 启动 → hello → Initialize → 兼容判定。
//!
//! `connect_initialized` 只做握手与 Initialize；连接编排（状态发布、尝试代次）留在
//! `source_server_connections`，更新探针也复用同一路径。

use anyhow::Result;

use crate::source_server_compatibility::{CompatibilityEvaluation, evaluate};
use crate::source_server_connection::{ConnectOptions, RemoteConnection};
use crate::source_server_connections::ConnectionsOptions;
use crate::source_server_transport::{TransportCommand, is_host_key_changed_error};

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
    command: &TransportCommand,
) -> std::result::Result<RemoteConnection, OpenFailure> {
    let connect_options = ConnectOptions {
        client_id: &options.client_id,
        client_version: &options.client_version,
        handshake_timeout: options.initialize_timeout,
    };
    let connection = crate::source_server_connection::connect(command, &connect_options)
        .await
        .map_err(|error| {
            if is_host_key_changed_error(&format!("{error:#}")) {
                OpenFailure::HostKeyChanged(error)
            } else {
                OpenFailure::Other(error)
            }
        })?;
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
    connect_initialized(options, command)
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
    command: &TransportCommand,
) -> std::result::Result<OpenedConnection, OpenFailure> {
    let connection = connect_initialized(options, command).await?;
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
