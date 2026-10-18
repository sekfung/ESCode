//! `git-checkpoint` 端到端夹具：真临时 git 仓库 + 已启动的 Host surface。
//! 被 `rpc_git_checkpoint.rs`（主流程）与 `rpc_git_checkpoint_lifecycle.rs`（删除与错误文案）共用。

use std::path::PathBuf;

use super::{RpcSurface, Wire};

pub fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 在 `cwd` 跑一条 git 命令，失败即 panic（**夹具**用，与 Host 的 git 调用无关）。
pub fn git(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Checkpoint Test")
        .env("GIT_AUTHOR_EMAIL", "test@escode.local")
        .env("GIT_COMMITTER_NAME", "Checkpoint Test")
        .env("GIT_COMMITTER_EMAIL", "test@escode.local")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} 失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn write(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).unwrap();
}

pub fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// RPC 调用的共用形状：一个对象参数。
pub fn params(value: serde_json::Value) -> Wire {
    Wire::Array(vec![Wire::Object(value)])
}

pub async fn error_of(socket: &mut super::Socket, id: i64) -> serde_json::Value {
    super::read_response(socket, id)
        .await
        .into_object()
        .expect("error payload")
}

/// 一个已 `git init` 并提交了 `a.txt`（内容 `one\n`）的临时仓库，外加已启动的 Host surface。
pub struct RepoFixture {
    pub _dir: tempfile::TempDir,
    pub workspace: PathBuf,
    /// 归一化后的绝对路径（`git rev-parse --show-toplevel` 口吻）。
    pub workspace_path: String,
    pub data_base_dir: PathBuf,
    pub surface: RpcSurface,
}

impl RepoFixture {
    /// `None` = 环境里没有 git 二进制（用例应跳过而不是失败）。
    pub async fn new() -> Option<Self> {
        if !git_available() {
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let data_base_dir = dir.path().join("data");
        let repo = dir.path().join("repo");
        for path in [&home, &data_base_dir, &repo] {
            std::fs::create_dir_all(path).unwrap();
        }
        git(&repo, &["init", "-q"]);
        write(&repo.join("a.txt"), "one\n");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        // `rev-parse --show-toplevel` 回真实路径：夹具先归一化，否则 repoRoot 与入参比对会假失败。
        let workspace = std::fs::canonicalize(&repo).unwrap();
        // 修复：Windows 上 canonicalize 回 `\\?\C:\...`（verbatim 前缀），Node 的 realpath 与 git 都不用
        // 这种形式；去掉非 UNC 的 verbatim 前缀，得到与 TS 一致的 `C:\...`（windows-latest host 测试）。
        #[cfg(windows)]
        let workspace = match workspace
            .to_str()
            .and_then(|text| text.strip_prefix(r"\\?\"))
        {
            Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
            _ => workspace,
        };
        let workspace_path = workspace.to_string_lossy().into_owned();
        let surface = super::start_with_data_base_dir(&home, &data_base_dir).await;
        Some(Self {
            _dir: dir,
            workspace,
            workspace_path,
            data_base_dir,
            surface,
        })
    }

    pub fn manifest_root(&self) -> PathBuf {
        self.data_base_dir
            .join(".escode")
            .join("v2")
            .join("checkpoints")
    }

    pub fn temp_index_root(&self) -> PathBuf {
        self.data_base_dir
            .join(".escode")
            .join("git-checkpoint-index")
    }

    /// 本 workspace 的 manifest 目录。夹具里只有一个工作区，故取根下唯一的子目录
    /// （**不**重新实现 `workspaceHash`：那是被测逻辑，算法由 host-services 的单测钉住）。
    pub fn manifest_dir(&self) -> Option<PathBuf> {
        std::fs::read_dir(self.manifest_root())
            .ok()?
            .flatten()
            .next()
            .map(|entry| entry.path())
    }

    /// 临时 index 目录必须被清干净（每次 createCheckpoint 一个 `index-*`，结束即删）。
    pub fn assert_temp_index_clean(&self) {
        let leftovers: Vec<_> = std::fs::read_dir(self.temp_index_root())
            .map(|entries| entries.flatten().collect())
            .unwrap_or_default();
        assert!(
            leftovers.is_empty(),
            "临时 index 目录必须清干净: {leftovers:?}"
        );
    }
}
