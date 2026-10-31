//! 更新安装事务（S4-d，docs/specs/source-server-remote-rust-host.md §11.4）。
//!
//! 部署锁覆盖「备份 → pending marker → 组件替换 → 提交/回滚」。只有新 server 完成握手后
//! 调用方才 commit；握手失败时回滚旧 release，避免把不可启动的版本留在远端。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::source_server_bootstrap::{PreparedInstall, classify_install_failure};
use crate::source_server_deploy_lock::{AcquireOptions, DeployLock, acquire};
use crate::source_server_exec::RemoteRunner;
use crate::source_server_install::{INSTALL_TIMEOUT, InstallMode, install_plan};
use crate::source_server_release::{ComponentMeta, REMOTE_ASSET_COMPONENT_META_DIR};
use crate::source_server_transport::{quote_posix_path_arg, quote_posix_shell_arg};

const UPDATE_BACKUP_DIR: &str = "~/.escode/.update-backup-";
const BACKUP_TIMEOUT: Duration = INSTALL_TIMEOUT;

/// 更新安装事务：成功后由调用方 commit，启动失败时 rollback。
pub struct UpdateInstallTransaction {
    runner: Arc<dyn RemoteRunner>,
    lock: DeployLock,
    backup: String,
}

impl UpdateInstallTransaction {
    pub async fn apply(
        runner: Arc<dyn RemoteRunner>,
        prepared: &PreparedInstall,
        lock: &AcquireOptions,
    ) -> Result<Self> {
        let lock = acquire(runner.as_ref(), lock).await?;
        let backup = format!("{UPDATE_BACKUP_DIR}{}", uuid::Uuid::new_v4());
        let install = async {
            create_backup(runner.as_ref(), &backup).await?;
            mark_refresh_pending(runner.as_ref(), prepared).await?;
            install_plan(runner.as_ref(), &prepared.plan, &prepared.mode)
                .await
                .map_err(|error| map_install_error(prepared, error))
        }
        .await;
        if let Err(error) = install {
            let restore = restore_backup(runner.as_ref(), &backup).await;
            let release = lock.release().await;
            if let Err(restore) = restore {
                return Err(anyhow!(
                    "{error:#}; restore previous release failed: {restore:#}"
                ));
            }
            if let Err(release) = release {
                return Err(anyhow!(
                    "{error:#}; release deploy lock failed: {release:#}"
                ));
            }
            return Err(error);
        }
        Ok(Self {
            runner,
            lock,
            backup,
        })
    }

    /// 新 server 已验证：删除旧 release 备份并释放锁。清理失败只记录，不把已生效的更新判为失败。
    pub async fn commit(self) {
        if let Err(error) = discard_backup(self.runner.as_ref(), &self.backup).await {
            escode_host_control::logging::warn(
                "source-server",
                format!("discard update backup {}: {error:#}", self.backup),
            );
        }
        if let Err(error) = self.lock.release().await {
            escode_host_control::logging::warn(
                "source-server",
                format!("release deploy lock after update: {error:#}"),
            );
        }
    }

    /// 新 server 启动/握手失败：恢复旧 release 后再释放锁；恢复错误随原错误一起上抛。
    pub async fn rollback(self) -> Result<()> {
        let restore = restore_backup(self.runner.as_ref(), &self.backup).await;
        let release = self.lock.release().await;
        match (restore, release) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }
}

fn map_install_error(prepared: &PreparedInstall, error: anyhow::Error) -> anyhow::Error {
    let scope = match prepared.mode {
        InstallMode::LocalUpload { .. } => "upload",
        InstallMode::RemoteDownload { .. } => "download",
    };
    anyhow!(
        "[source-server:{scope}] {}: {error:#}",
        classify_install_failure(&error)
    )
}

async fn create_backup(runner: &dyn RemoteRunner, backup: &str) -> Result<()> {
    let script = format!(
        "set -eu\nroot=~/.escode/server\nbackup={}\nrm -rf \"$backup\"\nif [ -d \"$root\" ]; then cp -Rp \"$root\" \"$backup\"; fi",
        quote_posix_path_arg(backup)
    );
    runner
        .run(&script, None, BACKUP_TIMEOUT)
        .await
        .context("backup current source server release")?;
    Ok(())
}

async fn restore_backup(runner: &dyn RemoteRunner, backup: &str) -> Result<()> {
    let script = format!(
        "set -eu\nroot=~/.escode/server\nbackup={}\nif [ -d \"$backup\" ]; then rm -rf \"$root\"; cp -Rp \"$backup\" \"$root\"; fi",
        quote_posix_path_arg(backup)
    );
    runner
        .run(&script, None, BACKUP_TIMEOUT)
        .await
        .context("restore previous source server release")?;
    Ok(())
}

async fn discard_backup(runner: &dyn RemoteRunner, backup: &str) -> Result<()> {
    let script = format!("rm -rf {}", quote_posix_path_arg(backup));
    runner
        .run(&script, None, BACKUP_TIMEOUT)
        .await
        .context("discard source server update backup")?;
    Ok(())
}

/// 内容寻址的 glm 在 server 先更新时保留强刷标记；成功安装后由最终 component meta 覆盖。
async fn mark_refresh_pending(runner: &dyn RemoteRunner, prepared: &PreparedInstall) -> Result<()> {
    if !prepared
        .plan
        .components
        .iter()
        .any(|component| component.component.id == "glm")
    {
        return Ok(());
    }
    let meta = ComponentMeta {
        id: "glm".to_owned(),
        version: None,
        sha256: None,
        pending_refresh_app_version: Some(prepared.manifest.app_version.clone()),
        platform_arch: prepared.manifest.platform_arch.clone(),
    };
    let script = format!(
        "set -eu\nmkdir -p {dir}\nprintf %s {content} > {path}",
        dir = quote_posix_path_arg(REMOTE_ASSET_COMPONENT_META_DIR),
        content = quote_posix_shell_arg(&meta.to_json_line()?),
        path = quote_posix_path_arg(&ComponentMeta::path("glm"))
    );
    runner
        .run(&script, None, Duration::from_secs(30))
        .await
        .context("mark glm refresh pending")?;
    Ok(())
}
