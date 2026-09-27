//! 与 Node `fs.realpath` 对齐的路径解析。
//! Windows 上 `std::fs::canonicalize` 返回 `\\?\C:\...` verbatim 路径：写进 prompt、工具输出和 Host 路径比较时
//! 与 TS（`C:\...`）不一致，Git Bash 也不能把它当 cwd。所有 realpath 必须经这里，避免两种形态混用。
use std::path::{Path, PathBuf};

pub async fn realpath(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    tokio::fs::canonicalize(path).await.map(simplify)
}

/// 尚不存在的路径的规范形式：对最近的已存在祖先取 realpath，再接回缺失的部分。读写状态键与检查点用它，
/// 与已存在文件的 realpath 同一形态（macOS /var→/private/var、Windows 8.3 短名）；全不存在时原样返回。
pub async fn realpath_for_create(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = realpath(current).await {
            return missing.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_owned());
                current = parent;
            }
            _ => return path.to_owned(),
        }
    }
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
