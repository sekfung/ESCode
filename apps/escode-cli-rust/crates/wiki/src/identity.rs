//! 项目身份解析的文件系统部分（spec §4）。规则本身在 `domain::wiki::project`；
//! 这里只做 realpath、向上找 `.git`、读 gitfile / commondir / config。任何一步失败都回退到更保守的身份，
//! 不会把两个项目合并。
use escode_cli_domain::wiki::project::{
    IdentityKind, ProjectIdentity, display_name, normalize_remote, parse_gitfile, path_key,
    pick_remote, project_id,
};
use std::path::{Path, PathBuf};

pub async fn resolve_project(
    workspace: &Path,
    workspace_identity: Option<&str>,
) -> ProjectIdentity {
    let absolute = std::path::absolute(workspace).unwrap_or_else(|_| workspace.to_owned());
    // 符号链接：解析到真实路径，同一目录的不同链接得到同一项目。
    // 修复：Windows 上裸 canonicalize 返回 `\\?\C:\...` verbatim 形态，项目根带前缀进入 Wiki 存储路径与展示
    // （storage_layout 在 Windows 失败）；统一走 host::realpath（去前缀，见 crates/host/src/realpath.rs 约定）。
    let real = escode_cli_host::realpath(&absolute)
        .await
        .unwrap_or(absolute);
    let insensitive = case_insensitive(&real).await;
    if let Some((root, git_dir)) = find_git(&real).await {
        let common = common_dir(&git_dir).await;
        let config = tokio::fs::read_to_string(common.join("config"))
            .await
            .unwrap_or_default();
        let root_text = root.to_string_lossy().into_owned();
        if let Some(remote) = pick_remote(&config).as_deref().and_then(normalize_remote) {
            let key = format!("git-remote:{remote}");
            let name = display_name(&remote);
            return identity(name, key, IdentityKind::GitRemote, root_text);
        }
        let common_key = path_key(&common.to_string_lossy(), insensitive);
        let name = display_name(&root_text);
        return identity(
            name,
            format!("git-local:{common_key}"),
            IdentityKind::GitLocal,
            root_text,
        );
    }
    let root_text = real.to_string_lossy().into_owned();
    match workspace_identity.map(str::trim).filter(|s| !s.is_empty()) {
        Some(explicit) => identity(
            display_name(&root_text),
            format!("workspace:{explicit}"),
            IdentityKind::WorkspaceIdentity,
            root_text,
        ),
        None => identity(
            display_name(&root_text),
            format!("path:{}", path_key(&root_text, insensitive)),
            IdentityKind::Path,
            root_text,
        ),
    }
}

fn identity(name: String, key: String, kind: IdentityKind, root: String) -> ProjectIdentity {
    ProjectIdentity {
        id: project_id(&name, &key),
        name,
        kind,
        key,
        root,
    }
}

/// 最近的 `.git`（目录或 gitfile）及其所在工作树根；嵌套仓库时内层优先。
async fn find_git(start: &Path) -> Option<(PathBuf, PathBuf)> {
    for dir in start.ancestors() {
        let candidate = dir.join(".git");
        let Ok(meta) = tokio::fs::metadata(&candidate).await else {
            continue;
        };
        if meta.is_dir() {
            return Some((dir.to_owned(), candidate));
        }
        if meta.is_file() {
            let text = tokio::fs::read_to_string(&candidate).await.ok()?;
            let target = parse_gitfile(&text)?;
            let path = PathBuf::from(&target);
            let path = if path.is_absolute() {
                path
            } else {
                dir.join(path)
            };
            let path = escode_cli_host::realpath(&path).await.unwrap_or(path);
            return Some((dir.to_owned(), path));
        }
    }
    None
}

/// 工作树的 gitdir 带 `commondir` 时归并到主仓库目录（多工作树共享项目）。
async fn common_dir(git_dir: &Path) -> PathBuf {
    let Ok(text) = tokio::fs::read_to_string(git_dir.join("commondir")).await else {
        return git_dir.to_owned();
    };
    let target = PathBuf::from(text.trim());
    let path = if target.is_absolute() {
        target
    } else {
        git_dir.join(target)
    };
    escode_cli_host::realpath(&path).await.unwrap_or(path)
}

/// 文件系统是否大小写不敏感：Windows 恒定；其余平台探测切换大小写后是否指向同一文件。
async fn case_insensitive(path: &Path) -> bool {
    if cfg!(windows) {
        return true;
    }
    let text = path.to_string_lossy();
    let swapped: String = text
        .chars()
        .map(|c| {
            if c.is_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    if swapped == text {
        return false;
    }
    let (Ok(a), Ok(b)) = (
        tokio::fs::metadata(path).await,
        tokio::fs::metadata(&swapped).await,
    ) else {
        return false;
    };
    same_file(&a, &b)
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}
