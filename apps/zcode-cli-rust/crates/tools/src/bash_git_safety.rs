//! git 运行时上下文安全判定：逐位对应 TS `bash-git-runtime-safety.ts`。
//! 工作目录（或其父目录）指向仓外 git 目录、或存在裸仓库标识时，git 可能加载非本仓的
//! hooks/config，因此即使命令本身只读也不能放行。
use std::path::{Path, PathBuf};

/// 组合入口：本模块负责文件系统事实，纯决策交给 domain（架构检查禁止 domain 做 IO）。
pub fn is_readonly_in_context(command: &str, cwd: Option<&Path>) -> bool {
    zcode_cli_domain::bash_policy::is_readonly_with_git_context(
        command,
        is_runtime_context_unsafe(cwd),
    )
}

const MAX_GITDIR_FILE_BYTES: u64 = 32 * 1024;

enum DotGit {
    None,
    Trusted,
    Unsafe,
}

pub fn is_runtime_context_unsafe(working_directory: Option<&Path>) -> bool {
    let Some(cwd) = working_directory else {
        return false;
    };
    let Some(cwd_canonical) = canonical(cwd) else {
        return true;
    };
    match classify_dot_git(cwd, &cwd_canonical) {
        DotGit::Trusted => return false,
        DotGit::Unsafe => return true,
        DotGit::None => {}
    }
    let mut current = cwd.to_path_buf();
    loop {
        if has_bare_git_indicators(&current) {
            return true;
        }
        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            return false;
        };
        if parent == current {
            return false;
        }
        match classify_dot_git(&parent, &cwd_canonical) {
            DotGit::Trusted => return false,
            DotGit::Unsafe => return true,
            DotGit::None => {}
        }
        current = parent;
    }
}

fn classify_dot_git(directory: &Path, cwd_canonical: &str) -> DotGit {
    let dot_git = directory.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
        return DotGit::None;
    };
    if meta.file_type().is_symlink() {
        return match std::fs::read_link(&dot_git) {
            Ok(target) => classify_target(&resolve_relative(directory, &target), cwd_canonical),
            Err(_) => DotGit::Unsafe,
        };
    }
    if meta.is_file() {
        if meta.len() > MAX_GITDIR_FILE_BYTES {
            return DotGit::Unsafe;
        }
        let Ok(content) = std::fs::read_to_string(&dot_git) else {
            return DotGit::None;
        };
        if content.contains('\0') {
            return DotGit::Unsafe;
        }
        let Some(rest) = content.strip_prefix("gitdir: ") else {
            return DotGit::None;
        };
        let target = rest.trim_end_matches(['\r', '\n']);
        let target = PathBuf::from(target);
        return classify_target(&resolve_relative(directory, &target), cwd_canonical);
    }
    if meta.is_dir() {
        return if has_trusted_git_directory(&dot_git) {
            DotGit::Trusted
        } else {
            DotGit::None
        };
    }
    DotGit::None
}

fn classify_target(target: &Path, cwd_canonical: &str) -> DotGit {
    let Some(canonical_target) = canonical(target) else {
        return DotGit::Unsafe;
    };
    if is_same_or_inside(&canonical_target, cwd_canonical) {
        return DotGit::Unsafe;
    }
    if !has_git_segment(&canonical_target) {
        return DotGit::Unsafe;
    }
    if has_valid_git_head(Path::new(&canonical_target)) {
        DotGit::Trusted
    } else {
        DotGit::None
    }
}

fn resolve_relative(directory: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        directory.join(target)
    }
}

fn has_trusted_git_directory(directory: &Path) -> bool {
    if !has_valid_git_head(directory) {
        return false;
    }
    for child in ["objects", "refs"] {
        if !std::fs::metadata(directory.join(child)).is_ok_and(|m| m.is_dir()) {
            return false;
        }
    }
    // commondir 表示 worktree 共享主仓，按 none 处理。
    std::fs::metadata(directory.join("commondir")).is_err()
}

fn has_valid_git_head(directory: &Path) -> bool {
    let head_path = directory.join("HEAD");
    let Ok(meta) = std::fs::symlink_metadata(&head_path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > 4096 {
        return false;
    }
    let Ok(head) = std::fs::read_to_string(&head_path) else {
        return false;
    };
    let head: String = head.chars().take(255).collect();
    head.starts_with("ref:") && head[4..].trim_start_matches([' ', '\t']).starts_with("refs/")
        || is_hex_object_id(&head)
}

fn is_hex_object_id(text: &str) -> bool {
    let text = text.trim_end_matches([' ', '\t', '\n', '\r']);
    (text.len() == 40 || text.len() == 64)
        && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn has_bare_git_indicators(directory: &Path) -> bool {
    if std::fs::symlink_metadata(directory.join("HEAD"))
        .is_ok_and(|m| m.is_file() || m.file_type().is_symlink())
    {
        return true;
    }
    ["objects", "refs"].iter().any(|c| directory.join(c).exists())
}

/// 对应 TS `canonicalPath`：realpath → `\` 归一为 `/` → NFC → 小写。
/// 已知差异：未做 NFC 归一（TS 的 `.normalize("NFC")`）。仅当路径含分解形式的重音字符时结果可能不同，
/// 影响限于 macOS 上的非 ASCII 路径；补上需要引入 unicode-normalization 依赖，另行评估。
fn canonical(path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let real = std::fs::canonicalize(absolute).ok()?;
    let text = zcode_cli_protocol::simplify_verbatim(&real.to_string_lossy())
        .unwrap_or_else(|| real.to_string_lossy().into_owned());
    Some(text.replace('\\', "/").to_lowercase())
}

fn is_same_or_inside(path: &str, base: &str) -> bool {
    let base_with_sep = if base.ends_with('/') {
        base.to_owned()
    } else {
        format!("{base}/")
    };
    path == base || path.starts_with(&base_with_sep)
}

fn has_git_segment(path: &str) -> bool {
    path.split(['\\', '/'])
        .any(|segment| segment.eq_ignore_ascii_case(".git"))
}
