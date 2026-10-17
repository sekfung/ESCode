//! `git-checkpoint` 服务层（TS `gitCheckpointService.ts`）：manifest 与 git 管道的组合，以及**错误语义**。
//!
//! 错误文案是可见行为（Renderer 直接展示），逐字对齐 TS（`rust-u2-git.md` §8.2）。
//! 特别注意 `Checkpoint workspace mismatch` 带的是 **meta 里的 checkpointId**，不是入参。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use super::GitService;
use super::checkpoint::{self, FileDiff, RestoreResult};
use super::checkpoint_store::{GitCheckpointMeta, GitCheckpointStore, checkpoints_dir};

pub struct GitCheckpointService {
    service: Arc<GitService>,
    store: GitCheckpointStore,
    data_base_dir: PathBuf,
}

/// TS `ensureComparableCheckpoints`：两条 checkpoint 必须同仓库、同 scope、同工作区边界。
fn ensure_comparable(left: &GitCheckpointMeta, right: &GitCheckpointMeta) -> Result<()> {
    if left.repo_root != right.repo_root {
        bail!("Checkpoint repoRoot mismatch.");
    }
    if left.scope != right.scope {
        bail!("Checkpoint scope mismatch.");
    }
    if left.workspace_in_repo_path != right.workspace_in_repo_path {
        bail!("Checkpoint workspace scope mismatch.");
    }
    Ok(())
}

impl GitCheckpointService {
    pub fn new(service: Arc<GitService>, data_base_dir: &Path) -> Self {
        Self {
            service,
            store: GitCheckpointStore::new(checkpoints_dir(data_base_dir)),
            data_base_dir: data_base_dir.to_path_buf(),
        }
    }

    /// TS `loadCheckpoint`：存在性 → 归属校验（`workspacePath` 必须逐字相同）。
    async fn load(&self, workspace_path: &str, checkpoint_id: &str) -> Result<GitCheckpointMeta> {
        let meta = self
            .store
            .load(workspace_path, checkpoint_id)
            .await
            .ok_or_else(|| anyhow!("Checkpoint does not exist: {checkpoint_id}"))?;
        if meta.workspace_path != workspace_path {
            bail!("Checkpoint workspace mismatch: {}", meta.checkpoint_id);
        }
        Ok(meta)
    }

    /// TS `createCheckpoint`：生成 id → git 侧固化 → manifest 落盘。
    pub async fn create(&self, workspace_path: &str) -> Result<GitCheckpointMeta> {
        let checkpoint_id = uuid::Uuid::new_v4().to_string();
        let meta = checkpoint::create_checkpoint(
            &self.service,
            &self.data_base_dir,
            workspace_path,
            &checkpoint_id,
        )
        .await?;
        self.store.save(&meta).await?;
        Ok(meta)
    }

    pub async fn diff(
        &self,
        workspace_path: &str,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
    ) -> Result<Vec<FileDiff>> {
        let (from, to) = self
            .load_pair(workspace_path, from_checkpoint_id, to_checkpoint_id)
            .await?;
        checkpoint::compute_diff(&self.service, workspace_path, &from, &to).await
    }

    pub async fn restore(
        &self,
        workspace_path: &str,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
        force: bool,
    ) -> Result<RestoreResult> {
        let (from, to) = self
            .load_pair(workspace_path, from_checkpoint_id, to_checkpoint_id)
            .await?;
        checkpoint::restore_between(&self.service, workspace_path, &from, &to, force).await
    }

    pub async fn delete(&self, workspace_path: &str, checkpoint_id: &str) -> Result<()> {
        let checkpoint = self.load(workspace_path, checkpoint_id).await?;
        checkpoint::delete_checkpoint_ref(&self.service, workspace_path, &checkpoint).await?;
        self.store.delete(workspace_path, checkpoint_id).await?;
        Ok(())
    }

    /// TS 两处 `Promise.all([loadCheckpoint(from), loadCheckpoint(to)])`：
    /// 并发加载，任一失败即失败（`from` 的错误优先返回）。
    async fn load_pair(
        &self,
        workspace_path: &str,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
    ) -> Result<(GitCheckpointMeta, GitCheckpointMeta)> {
        let (from, to) = tokio::join!(
            self.load(workspace_path, from_checkpoint_id),
            self.load(workspace_path, to_checkpoint_id),
        );
        let (from, to) = (from?, to?);
        ensure_comparable(&from, &to)?;
        Ok((from, to))
    }
}
