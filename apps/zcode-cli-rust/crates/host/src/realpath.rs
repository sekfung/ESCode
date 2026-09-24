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

// verbatim 前缀归一由展示层共用（domain 的 git 安全判定也要比较路径），实现放在 protocol。
pub use zcode_cli_protocol::simplify_verbatim;

#[cfg(test)]
mod tests {
    #[test]
    fn realpath_resolves_the_current_directory() {
        let resolved = super::realpath_sync(".").unwrap();
        assert!(resolved.is_absolute());
    }
}
