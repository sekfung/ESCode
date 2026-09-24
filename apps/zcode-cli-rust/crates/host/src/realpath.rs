//! 与 Node `fs.realpath` 对齐的路径解析。
//! Windows 上 `std::fs::canonicalize` 返回 `\\?\C:\...` verbatim 路径：写进 prompt、工具输出和 Host 路径比较时
//! 与 TS（`C:\...`）不一致，Git Bash 也不能把它当 cwd。所有 realpath 必须经这里，避免两种形态混用。
use std::path::{Path, PathBuf};

pub async fn realpath(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    tokio::fs::canonicalize(path).await.map(simplify)
}

pub fn realpath_sync(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify)
}

fn simplify(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    match simplify_verbatim(&path.to_string_lossy()) {
        Some(plain) => PathBuf::from(plain),
        None => path,
    }
}

/// `\\?\C:\x` → `C:\x`，`\\?\UNC\srv\share\x` → `\\srv\share\x`；其他 verbatim 形态（如 `\\?\Volume{..}`）
/// 没有等价的普通路径，保持原样。
pub fn simplify_verbatim(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = path.strip_prefix(r"\\?\")?;
    let bytes = rest.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        .then(|| rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::simplify_verbatim;

    #[test]
    fn strips_verbatim_prefix_like_node_realpath() {
        assert_eq!(simplify_verbatim(r"\\?\C:\a\b").as_deref(), Some(r"C:\a\b"));
        assert_eq!(
            simplify_verbatim(r"\\?\UNC\srv\share\x").as_deref(),
            Some(r"\\srv\share\x")
        );
        assert_eq!(simplify_verbatim(r"\\?\Volume{1}\x"), None);
        assert_eq!(simplify_verbatim(r"C:\plain"), None);
    }
}
