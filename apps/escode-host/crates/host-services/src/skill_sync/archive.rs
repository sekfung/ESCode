//! skill 同步归档：gzip + ustar（TS `skillSyncArchive.ts`）。
//!
//! 归档是本地/远端两个 Host 之间的事实：写入方与读取方可能一边是 TS、一边是 Rust，
//! 因此头字段与条目类型严格按 TS 实现，读取侧只接受目录与普通文件。

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use super::archive_tar::{self, BLOCK};
use super::error::SkillSyncError;
use super::paths;

/// 一个待写入归档的目录（TS `SkillSyncArchiveEntry`）。
pub struct ArchiveEntry {
    pub source_path: PathBuf,
    pub archive_path: String,
}

/// TS `createSkillSyncArchive`：目录按名字排序递归；符号链接等特殊文件直接报错。
pub fn create_archive(
    entries: &[ArchiveEntry],
    compare: &crate::skills::Compare,
) -> Result<Vec<u8>, SkillSyncError> {
    let mut tar = Vec::new();
    for entry in entries {
        let archive_path =
            paths::normalize_relative(&entry.archive_path, "unsafe skill archive path")?;
        append_tree(&mut tar, &entry.source_path, &archive_path, compare)?;
    }
    tar.extend_from_slice(&[0_u8; BLOCK * 2]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).map_err(SkillSyncError::from)?;
    encoder.finish().map_err(SkillSyncError::from)
}

fn append_tree(
    tar: &mut Vec<u8>,
    source_path: &Path,
    archive_path: &str,
    compare: &crate::skills::Compare,
) -> Result<(), SkillSyncError> {
    let metadata = std::fs::symlink_metadata(source_path)?;
    if metadata.is_dir() {
        let directory_path = format!("{}/", archive_path.trim_end_matches('/'));
        tar.extend_from_slice(&archive_tar::header(
            &directory_path,
            mode(&metadata),
            mtime(&metadata),
            0,
            b'5',
        )?);
        let mut children: Vec<std::fs::DirEntry> =
            std::fs::read_dir(source_path)?.collect::<Result<_, _>>()?;
        children.sort_by(|left, right| {
            compare(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            let child_path = paths::normalize_relative(
                &format!("{archive_path}/{name}"),
                "unsafe skill archive path",
            )?;
            append_tree(tar, &child.path(), &child_path, compare)?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        let content = std::fs::read(source_path)?;
        tar.extend_from_slice(&archive_tar::header(
            archive_path,
            mode(&metadata),
            mtime(&metadata),
            content.len() as u64,
            b'0',
        )?);
        tar.extend_from_slice(&content);
        let padding = (archive_tar::padded(content.len() as u64)? - content.len() as u64) as usize;
        tar.extend(std::iter::repeat_n(0_u8, padding));
        return Ok(());
    }
    Err(SkillSyncError::plain(format!(
        "unsupported skill archive source: {}",
        source_path.display()
    )))
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() as u64
}

#[cfg(not(unix))]
fn mode(metadata: &std::fs::Metadata) -> u64 {
    if metadata.is_dir() { 0o755 } else { 0o644 }
}

#[cfg(unix)]
fn mtime(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    u64::try_from(metadata.mtime()).unwrap_or(0)
}

#[cfg(not(unix))]
fn mtime(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// TS `extractSkillSyncArchive`：流式解压，按文件累计上限拒绝压缩炸弹。
pub fn extract_archive(
    archive: &[u8],
    target_dir: &Path,
    max_extracted_bytes: Option<u64>,
) -> Result<(), SkillSyncError> {
    std::fs::create_dir_all(target_dir)?;
    let target_root = std::fs::canonicalize(target_dir)?;
    let mut reader = BufReader::new(GzDecoder::new(archive));
    let mut extracted: u64 = 0;
    while let Some(header) = read_block(&mut reader)? {
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = archive_tar::parse_octal(&header[124..136])?;
        let type_flag = {
            let value = archive_tar::read_string(&header[156..157]);
            if value.is_empty() {
                "0".to_owned()
            } else {
                value
            }
        };
        let archive_path = paths::normalize_relative(
            &archive_tar::entry_path(&header),
            "unsafe skill archive path",
        )?;
        let target =
            paths::resolve_within(&target_root, &archive_path, "unsafe skill archive path")?;
        if type_flag == "5" {
            skip_exact(&mut reader, archive_tar::padded(size)?)?;
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if type_flag != "0" && type_flag != "\0" {
            return Err(SkillSyncError::plain(format!(
                "unsupported skill archive entry type: {type_flag}"
            )));
        }
        let next = extracted
            .checked_add(size)
            .ok_or_else(|| SkillSyncError::plain("invalid skill sync archive entry size"))?;
        if let Some(max) = max_extracted_bytes
            && next > max
        {
            return Err(SkillSyncError::size_limit(next, max, "extracted-content"));
        }
        let length = usize::try_from(size)
            .map_err(|_| SkillSyncError::plain("invalid skill sync archive entry size"))?;
        let mut data = vec![0_u8; length];
        reader.read_exact(&mut data).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                SkillSyncError::plain("truncated skill sync archive entry")
            } else {
                SkillSyncError::from(error)
            }
        })?;
        skip_exact(&mut reader, archive_tar::padded(size)? - size)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &data)?;
        extracted = next;
    }
    Ok(())
}

fn read_block(reader: &mut impl Read) -> Result<Option<[u8; BLOCK]>, SkillSyncError> {
    let mut buffer = [0_u8; BLOCK];
    let mut filled = 0;
    while filled < BLOCK {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(SkillSyncError::from(error)),
        }
    }
    match filled {
        0 => Ok(None),
        BLOCK => Ok(Some(buffer)),
        _ => Err(SkillSyncError::plain("truncated skill sync archive")),
    }
}

fn skip_exact(reader: &mut impl Read, bytes: u64) -> Result<(), SkillSyncError> {
    let mut remaining = bytes;
    let mut scratch = [0_u8; 8192];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(scratch.len() as u64)).unwrap_or(scratch.len());
        let count = reader.read(&mut scratch[..want])?;
        if count == 0 {
            return Err(SkillSyncError::plain("truncated skill sync archive"));
        }
        remaining -= count as u64;
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
