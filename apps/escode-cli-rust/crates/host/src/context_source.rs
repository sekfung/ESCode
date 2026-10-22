use crate::{
    contract::ContextPort,
    domain::prompt::{InstructionSource, PromptSnapshot},
};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub struct WorkspaceContext {
    cwd: PathBuf,
    home: PathBuf,
    desktop: bool,
    /// Host 经 `ESCODE_GEN_UI_OUTPUT_ROOT` 提供的输出根与工作区身份 key（identity || path）。
    gen_ui: Option<(PathBuf, String)>,
}
impl WorkspaceContext {
    pub fn new(cwd: PathBuf, home: PathBuf, desktop: bool) -> Self {
        Self {
            cwd,
            home,
            desktop,
            gen_ui: None,
        }
    }
    /// 输出根必须是绝对路径（TS getGenUiOutputDirectory 同一校验），否则不启用。
    pub fn with_gen_ui(mut self, root: Option<PathBuf>, workspace_key: String) -> Self {
        self.gen_ui = root
            .filter(|root| root.is_absolute())
            .map(|root| (root, workspace_key));
        self
    }
}

/// TS `getGenUiOutputDirectory`：`<root>/<sha256(JSON.stringify([workspaceKey, sessionId]))>`。
pub fn gen_ui_output_directory(root: &Path, workspace_key: &str, session_id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = serde_json::to_string(&[workspace_key, session_id]).unwrap_or_default();
    root.join(format!("{:x}", Sha256::digest(key.as_bytes())))
}
#[async_trait::async_trait]
impl ContextPort for WorkspaceContext {
    fn desktop(&self) -> bool {
        self.desktop
    }
    async fn gen_ui_directory(&self, session_id: &str) -> Option<String> {
        let (root, key) = self.gen_ui.as_ref().filter(|_| self.desktop)?;
        let directory = gen_ui_output_directory(root, key, session_id);
        // 只有成功创建后才向模型声明可写目录；失败不能让模型把可视化写进 Git 工作区。
        tokio::fs::create_dir_all(&directory).await.ok()?;
        Some(directory.to_string_lossy().into_owned())
    }
    async fn snapshot(&self, cancel: &CancellationToken) -> Result<PromptSnapshot> {
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => other,
        };
        // SVN 工作副本（且比 Git 仓库更近）时注入 SVN 快照，不再收集 Git 快照。
        let is_svn = super::context_svn::is_svn_workspace(&self.cwd, cancel).await;
        let vcs = async {
            if is_svn {
                (None, Some(super::context_svn::snapshot(&self.cwd, cancel).await))
            } else {
                (super::context_git::snapshot(&self.cwd, cancel).await, None)
            }
        };
        let ((git, svn), release) =
            tokio::join!(vcs, super::context_git::os_release(&self.cwd, cancel));
        anyhow::ensure!(!cancel.is_cancelled(), "Cancelled");
        Ok(PromptSnapshot {
            cwd: self.cwd.to_string_lossy().into_owned(),
            platform: platform.into(),
            shell: if cfg!(windows) { "cmd.exe" } else { "bash" }.into(),
            os_version: format!("{platform} {} {arch}", release.trim()),
            current_date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            git,
            svn,
        })
    }
    async fn instructions(&self, cancel: &CancellationToken) -> Result<Vec<InstructionSource>> {
        tokio::select! { biased;
            _=cancel.cancelled()=>anyhow::bail!("Cancelled"),
            sources=read_instructions(&self.cwd, &self.home)=>Ok(sources),
        }
    }
}
async fn read_instructions(cwd: &Path, home: &Path) -> Vec<InstructionSource> {
    // TS 只选择最近文件，并在最近 Git 根停止；不能把父目录或隐藏候选全量拼接进请求。
    let mut candidates = vec![(home.join(".escode").join("AGENTS.md"), true)];
    for directory in cwd.ancestors() {
        let candidate = directory.join("AGENTS.md");
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|m| m.is_file())
        {
            if candidate != candidates[0].0 {
                candidates.push((candidate, false));
            }
            break;
        }
        if tokio::fs::metadata(directory.join(".git")).await.is_ok() {
            break;
        }
    }
    let mut sources = vec![];
    for (path, user) in candidates {
        if let Ok(source) = read_source(&path, user).await {
            sources.push(source);
        }
    }
    sources
}
async fn read_source(path: &Path, user: bool) -> Result<InstructionSource> {
    const MAX_BYTES: u64 = 100 * 1024;
    let meta = tokio::fs::metadata(path).await?;
    anyhow::ensure!(meta.is_file(), "Instructions must be a regular file");
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::with_capacity(meta.len().min(MAX_BYTES) as usize);
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).await?;
    let truncated = meta.len() > MAX_BYTES || bytes.len() > MAX_BYTES as usize;
    bytes.truncate(MAX_BYTES as usize);
    Ok(InstructionSource {
        path: path.to_string_lossy().into_owned(),
        user,
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn gen_ui_directory_matches_ts_hash_and_requires_desktop_absolute_root() {
        // 期望值由 TS getGenUiOutputDirectory 的同一算法（sha256(JSON.stringify([key, sessionId]))）算出。
        let root = tempfile::tempdir().unwrap();
        let expected = root
            .path()
            .join("342b53d3df2fd7df8a08812a113b84ea456420a32ea3a2ce6d7aaa69e5d579a2");
        assert_eq!(
            gen_ui_output_directory(root.path(), "remote-1", "sess_abc"),
            expected
        );
        assert_eq!(
            gen_ui_output_directory(Path::new("/r"), "C:\repo \"x\"", "sess_1"),
            Path::new("/r")
                .join("72fede38cca1534af7c99b67039375d4472d00a1052cde28af12539b8ef2b23c")
        );
        let context = WorkspaceContext::new(root.path().into(), root.path().into(), true)
            .with_gen_ui(Some(root.path().into()), "remote-1".into());
        assert_eq!(
            context.gen_ui_directory("sess_abc").await,
            Some(expected.to_string_lossy().into_owned())
        );
        assert!(expected.is_dir());
        let cli = WorkspaceContext::new(root.path().into(), root.path().into(), false)
            .with_gen_ui(Some(root.path().into()), "remote-1".into());
        assert_eq!(cli.gen_ui_directory("sess_abc").await, None);
        let relative = WorkspaceContext::new(root.path().into(), root.path().into(), true)
            .with_gen_ui(Some(PathBuf::from("relative")), "remote-1".into());
        assert_eq!(relative.gen_ui_directory("sess_abc").await, None);
    }
    #[tokio::test]
    async fn same_user_and_workspace_path_is_injected_once_and_non_files_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join(".escode");
        tokio::fs::create_dir_all(cwd.join(".git")).await.unwrap();
        tokio::fs::write(cwd.join("AGENTS.md"), "one source")
            .await
            .unwrap();
        let sources = read_instructions(&cwd, root.path()).await;
        assert_eq!(sources.len(), 1);
        assert!(sources[0].user);
        assert_eq!(sources[0].content, "one source");
        tokio::fs::remove_file(cwd.join("AGENTS.md")).await.unwrap();
        tokio::fs::create_dir(cwd.join("AGENTS.md")).await.unwrap();
        tokio::fs::write(root.path().join("AGENTS.md"), "outside git boundary")
            .await
            .unwrap();
        assert!(read_instructions(&cwd, root.path()).await.is_empty());
    }
    #[tokio::test]
    async fn cancelled_context_initialization_does_not_return_a_snapshot_or_sources() {
        let root = tempfile::tempdir().unwrap();
        let source = WorkspaceContext::new(root.path().into(), root.path().into(), false);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(source.snapshot(&cancel).await.is_err());
        assert!(source.instructions(&cancel).await.is_err());
    }
}
