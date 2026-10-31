//! `source-server-management` 生产装配选项（S4-j）。
//!
//! 从 `domains_source_server.rs` 拆出以守住单文件 400 行门禁：transport 解析、凭据解密、
//! identity probe 与凭据服务只在这里组合。

use std::sync::Arc;

use escode_host_services::source_server_connections::ConnectionsOptions;

use super::DEFAULT_REMOTE_SERVER_SCRIPT;

/// Main 通过 `rpcListen` 交给 Host 的进程事实（Host 不读进程 env，见边界检查）。
#[derive(Clone, Debug)]
pub struct HostEnvironment {
    /// Host 可执行文件：transport 用它的 `--ssh-askpass` 模式读取密码/口令。
    pub askpass_program: std::path::PathBuf,
    /// hello-ack 的客户端版本（Main 的 app 版本，来自 `runtime_env.ESCODE_APP_VERSION`）。
    pub client_version: String,
    /// 首次安装用的远程资源事实（spec §11.3）；旧 Main 不交付时只启动 server。
    pub remote_assets: Option<escode_host_services::source_server_bootstrap::RemoteAssetsConfig>,
}

/// 生产 transport 解析：profile → ssh / wsl / docker 启动命令。
///
/// 密码/口令只从 `credential` 域解密后放进子进程 env（`--ssh-askpass`），secret 不进 argv。
pub fn default_connections_options(
    data_base_dir: Option<&std::path::Path>,
    host: Option<HostEnvironment>,
) -> ConnectionsOptions {
    let credentials = data_base_dir.map(|base| {
        Arc::new(escode_host_services::credential::CredentialService::new(
            escode_host_services::credential::CredentialService::path(base),
            &escode_cli_host::credential_cipher::credential_secret(),
        ))
    });
    let rekey_credentials = credentials.clone();
    let client_version = host
        .as_ref()
        .map(|host| host.client_version.clone())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    let askpass_program = host.as_ref().map(|host| host.askpass_program.clone());
    let known_hosts =
        data_base_dir.map(escode_host_services::source_server_transport::default_known_hosts_path);
    let install = host.and_then(|host| host.remote_assets).map(|assets| {
        escode_host_services::source_server_bootstrap::InstallOptions {
            assets,
            lock: escode_host_services::source_server_deploy_lock::AcquireOptions::default(),
        }
    });
    ConnectionsOptions::new(client_version, move |profile| {
        let credentials = credentials.clone();
        let askpass_program = askpass_program.clone();
        let known_hosts = known_hosts.clone();
        let profile = profile.clone();
        Box::pin(async move {
            use escode_host_services::source_server_transport::{
                TransportSecrets, TransportTarget, parse_target,
            };
            // askpass 只在 SSH 密码/口令路径被使用；缺 Host 路径时显式失败，不回退 agent 认证。
            let askpass_program = askpass_program.ok_or_else(|| {
                anyhow::anyhow!(
                    "source server transport needs the Host executable path: Main must pass it in rpcListen"
                )
            })?;
            let target = parse_target(&profile)?;
            let known_hosts = match &target {
                TransportTarget::Ssh(_) => {
                    let path = known_hosts.clone().ok_or_else(|| {
                        anyhow::anyhow!(
                            "source server SSH host-key store needs dataBaseDir: Main must pass dataBaseDir in rpcListen"
                        )
                    })?;
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|error| {
                            anyhow::anyhow!(
                                "create known_hosts directory {}: {error}",
                                parent.display()
                            )
                        })?;
                    }
                    Some(path)
                }
                _ => None,
            };
            let mut secrets = TransportSecrets::default();
            if let TransportTarget::Ssh(ssh) = &target {
                let store = |key: &str| {
                    credentials.clone().ok_or_else(|| {
                        anyhow::anyhow!(
                            "source server credentials need dataBaseDir: Main must pass dataBaseDir in rpcListen (credential {key})"
                        )
                    })
                };
                if let Some(key) = &ssh.password_credential_key {
                    let store = store(key)?;
                    secrets.password = store
                        .load(key)
                        .await
                        .map_err(|error| anyhow::anyhow!("load credential {key}: {error}"))?;
                }
                if let Some(key) = &ssh.private_key_passphrase_credential_key {
                    let store = store(key)?;
                    secrets.private_key_passphrase = store
                        .load(key)
                        .await
                        .map_err(|error| anyhow::anyhow!("load credential {key}: {error}"))?;
                }
            }
            let executor = std::sync::Arc::new(
                escode_host_services::source_server_exec::RemoteExecutor::new(
                    target,
                    secrets,
                    askpass_program,
                )
                .with_known_hosts(known_hosts),
            );
            let command = executor.build_command(DEFAULT_REMOTE_SERVER_SCRIPT)?;
            Ok(escode_host_services::source_server_exec::ResolvedTransport {
                command,
                runner: executor,
            })
        })
    })
    .with_install(install)
    .with_identity_probe(Some(Arc::new(|runner| {
        Box::pin(escode_host_services::source_server::probe_server_identity(
            runner,
            escode_host_services::source_server::IDENTITY_PROBE_TIMEOUT,
        ))
    })))
    .with_credentials(rekey_credentials)
}
