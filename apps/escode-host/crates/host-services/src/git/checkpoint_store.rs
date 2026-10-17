//! checkpoint manifest 存储（TS `gitCheckpointStore.ts`）。
//!
//! 位置：`<dataBaseDir>/.escode/v2/checkpoints/<workspaceHash>/<checkpointId>.json`。
//! `workspaceHash` = `sha256(workspacePath)` 十六进制前 12 位——**store 层不带 identity**，
//! 与 sessions 目录的 `workspaceKey` 口径不同（`rust-u2-git.md` §8.1，逐字复刻不"统一"）。
//!
//! 读校验是**浅校验**（TS `isGitCheckpointMeta`：七个必填字段的类型 + `scope === "workspace"`），
//! 且读取/解析/校验任一失败一律回 `None`（不抛）——损坏的 manifest 与"不存在"对调用方同义。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Number;
use sha2::{Digest, Sha256};

/// TS `GitCheckpointMeta`。字段声明序 = TS 对象字面量序 = 落盘 JSON 的键序（逐字节对齐 TS）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GitCheckpointMeta {
    #[serde(rename = "checkpointId")]
    pub checkpoint_id: String,
    #[serde(rename = "workspacePath")]
    pub workspace_path: String,
    #[serde(rename = "repoRoot")]
    pub repo_root: String,
    #[serde(rename = "workspaceInRepoPath")]
    pub workspace_in_repo_path: String,
    /// `Date.now()` 是整数，但校验口径是「是 number」（`1.5` 也合法）；
    /// 用 `Number` 而非 `f64` 是为了把读到的写法**原样写回**（`f64` 会把整数写成 `123.0`，与 TS 落盘不一致）。
    #[serde(rename = "createdAt")]
    pub created_at: Number,
    #[serde(rename = "refName")]
    pub ref_name: String,
    #[serde(rename = "commitOid")]
    pub commit_oid: String,
    pub scope: String,
}

impl GitCheckpointMeta {
    pub fn scope_is_workspace(&self) -> bool {
        self.scope == "workspace"
    }
}

/// TS `getWorkspaceHash(workspacePath)`：sha256 十六进制前 12 位。
pub fn workspace_hash(workspace_path: &str) -> String {
    let digest = Sha256::digest(workspace_path.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex.truncate(12);
    hex
}

/// TS `getAppConfigDir()` = `<dataBaseDir>/.escode/v2`。
pub fn app_config_dir(data_base_dir: &Path) -> PathBuf {
    data_base_dir.join(".escode").join("v2")
}

/// `<dataBaseDir>/.escode/v2/checkpoints`（TS `join(getAppConfigDir(), "checkpoints")`）。
pub fn checkpoints_dir(data_base_dir: &Path) -> PathBuf {
    app_config_dir(data_base_dir).join("checkpoints")
}

/// `<dataBaseDir>/.escode/git-checkpoint-index`（TS `getGitCheckpointIndexRootDir()`，**不在 `v2` 下**）。
pub fn temp_index_root(data_base_dir: &Path) -> PathBuf {
    data_base_dir.join(".escode").join("git-checkpoint-index")
}

/// checkpoint manifest 存储句柄（root 固定，per-file 路径由 workspace + id 派生）。
#[derive(Clone)]
pub struct GitCheckpointStore {
    root: PathBuf,
}

impl GitCheckpointStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn dir(&self, workspace_path: &str) -> PathBuf {
        self.root.join(workspace_hash(workspace_path))
    }

    fn file(&self, workspace_path: &str, checkpoint_id: &str) -> PathBuf {
        self.dir(workspace_path)
            .join(format!("{checkpoint_id}.json"))
    }

    /// 写 manifest。内容 = `JSON.stringify(meta, null, 2) + "\n"`（2 空格缩进 + 结尾换行）。
    pub async fn save(&self, meta: &GitCheckpointMeta) -> std::io::Result<()> {
        let path = self.file(&meta.workspace_path, &meta.checkpoint_id);
        let mut bytes = serde_json::to_vec_pretty(meta)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        let write = path.clone();
        tokio::task::spawn_blocking(move || escode_host_core::atomic_write::write(&write, &bytes))
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)
    }

    /// 读 manifest：不存在、坏 JSON、形状不符**一律 `None`**。
    pub async fn load(
        &self,
        workspace_path: &str,
        checkpoint_id: &str,
    ) -> Option<GitCheckpointMeta> {
        let raw = tokio::fs::read_to_string(self.file(workspace_path, checkpoint_id))
            .await
            .ok()?;
        let meta: GitCheckpointMeta = serde_json::from_str(&raw).ok()?;
        meta.scope_is_workspace().then_some(meta)
    }

    /// 删 manifest（force：文件不存在不是错误）。
    pub async fn delete(&self, workspace_path: &str, checkpoint_id: &str) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.file(workspace_path, checkpoint_id)).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
#[path = "checkpoint_store_tests.rs"]
mod tests;
