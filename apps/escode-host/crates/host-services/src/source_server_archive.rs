//! 远端资源 local-upload 归档（S4-c，docs/specs/source-server-remote-rust-host.md §11.3）。
//!
//! 复用 crate 内 ustar 头，把 release 目录中的文件/目录打成 gzip 后经 transport stdin 上传。
//! 归档内路径保持 release 布局（如 `node/linux-x64/node`），远端脚本按显式映射安装到最终路径。

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;

use crate::source_server_release::normalize_relative_path;
use crate::ustar::{self, BLOCK};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub source_path: PathBuf,
    pub archive_path: String,
}

/// 按显式 entry 列表生成 tar.gz；目录递归、文件保留权限位（Windows 源统一 0644）。
pub fn create_archive(entries: &[ArchiveEntry]) -> Result<Vec<u8>> {
    let mut tar = Vec::new();
    for entry in entries {
        let archive_path = normalize_relative_path(&entry.archive_path, "archive path")?;
        append_tree(&mut tar, &entry.source_path, &archive_path)?;
    }
    tar.extend_from_slice(&[0_u8; BLOCK * 2]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&tar)
        .context("gzip remote asset archive")?;
    encoder.finish().context("finish remote asset archive")
}

fn append_tree(tar: &mut Vec<u8>, source: &Path, archive_path: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("read release asset {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "release asset symlinks are not supported: {}",
            source.display()
        );
    }
    if metadata.is_dir() {
        let directory_path = format!("{}/", archive_path.trim_end_matches('/'));
        tar.extend_from_slice(
            &ustar::header(&directory_path, mode(&metadata), mtime(&metadata), 0, b'5')
                .map_err(anyhow::Error::msg)?,
        );
        let mut children: Vec<std::fs::DirEntry> = std::fs::read_dir(source)
            .with_context(|| format!("read release directory {}", source.display()))?
            .collect::<Result<_, _>>()?;
        children.sort_by_key(std::fs::DirEntry::file_name);
        for child in children {
            let child_archive = format!("{archive_path}/{}", child.file_name().to_string_lossy());
            append_tree(tar, &child.path(), &child_archive)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        bail!("unsupported release asset: {}", source.display());
    }
    let content =
        std::fs::read(source).with_context(|| format!("read release file {}", source.display()))?;
    tar.extend_from_slice(
        &ustar::header(
            archive_path,
            mode(&metadata),
            mtime(&metadata),
            content.len() as u64,
            b'0',
        )
        .map_err(anyhow::Error::msg)?,
    );
    tar.extend_from_slice(&content);
    let padding = ustar::padded(content.len() as u64).map_err(anyhow::Error::msg)? as usize;
    tar.extend(std::iter::repeat_n(0_u8, padding - content.len()));
    Ok(())
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::PermissionsExt;
    u64::from(metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode(_metadata: &std::fs::Metadata) -> u64 {
    0o644
}

fn mtime(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
#[path = "source_server_archive_tests.rs"]
mod tests;
