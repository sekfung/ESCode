//! plugin-sync 归档：gzip + ustar（TS `pluginSyncArchive.ts` 逐字节格式）。
//!
//! 支持两类条目：目录（type `5`，递归、子项按 localeCompare 排序）与普通文件（type `0`）；
//! 另支持直接在内存中写入文件（metadata / marketplace manifest）。解压只接受目录与普通文件。

use std::io::{Read, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

pub const METADATA_ARCHIVE_PATH: &str = ".escode-plugin-sync.json";
use super::archive_tar::{self, BLOCK};

pub enum ArchiveEntry {
    Source {
        source_path: std::path::PathBuf,
        archive_path: String,
    },
    Memory {
        archive_path: String,
        content: Vec<u8>,
        mode: u64,
        mtime_ms: Option<u64>,
    },
}

pub fn create_archive(
    entries: &[ArchiveEntry],
    metadata: &str,
    compare: &crate::skills::Compare,
) -> Result<Vec<u8>, String> {
    let mut tar = Vec::new();
    for entry in entries {
        match entry {
            ArchiveEntry::Source {
                source_path,
                archive_path,
            } => append_tree(&mut tar, source_path, archive_path, compare)?,
            ArchiveEntry::Memory {
                archive_path,
                content,
                mode,
                mtime_ms,
            } => append_file(
                &mut tar,
                archive_path,
                content,
                *mode,
                mtime_ms.unwrap_or_else(now_ms),
            )?,
        }
    }
    append_file(
        &mut tar,
        METADATA_ARCHIVE_PATH,
        metadata.as_bytes(),
        0o644,
        now_ms(),
    )?;
    tar.extend_from_slice(&[0_u8; BLOCK * 2]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).map_err(|error| error.to_string())?;
    encoder.finish().map_err(|error| error.to_string())
}

fn append_tree(
    tar: &mut Vec<u8>,
    source: &Path,
    archive_path: &str,
    compare: &crate::skills::Compare,
) -> Result<(), String> {
    let normalized = super::paths::normalize_relative(archive_path, "unsafe plugin archive path")?;
    let metadata = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.is_dir() {
        let directory_path = format!("{}/", normalized.trim_end_matches('/'));
        tar.extend_from_slice(&archive_tar::header(
            &directory_path,
            mode(&metadata),
            mtime_seconds(&metadata),
            0,
            b'5',
        )?);
        let mut children: Vec<std::fs::DirEntry> = std::fs::read_dir(source)
            .map_err(|error| error.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|error| error.to_string())?;
        children.sort_by(|left, right| {
            compare(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            let child_archive = format!("{normalized}/{name}");
            append_tree(tar, &child.path(), &child_archive, compare)?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        let content = std::fs::read(source).map_err(|error| error.to_string())?;
        append_file(
            tar,
            &normalized,
            &content,
            mode(&metadata),
            mtime_ms(&metadata),
        )?;
        return Ok(());
    }
    Err(format!(
        "unsupported plugin archive source: {}",
        source.display()
    ))
}

fn append_file(
    tar: &mut Vec<u8>,
    archive_path: &str,
    content: &[u8],
    mode: u64,
    mtime_ms: u64,
) -> Result<(), String> {
    let path = super::paths::normalize_relative(archive_path, "unsafe plugin archive path")?;
    tar.extend_from_slice(&archive_tar::header(
        &path,
        mode,
        mtime_ms / 1000,
        content.len() as u64,
        b'0',
    )?);
    tar.extend_from_slice(content);
    tar.extend(std::iter::repeat_n(
        0_u8,
        archive_tar::padded(content.len() as u64)? as usize - content.len(),
    ));
    Ok(())
}

pub fn extract_archive(
    archive: &[u8],
    target_dir: &Path,
    max_extracted_bytes: Option<u64>,
) -> Result<(), String> {
    std::fs::create_dir_all(target_dir).map_err(|error| error.to_string())?;
    let max_tar = max_extracted_bytes
        .map(|max| max + (BLOCK as u64 * 2) + (BLOCK as u64 * 512))
        .unwrap_or(u64::MAX);
    let mut decoder = GzDecoder::new(archive);
    let mut buffer = Vec::new();
    decoder
        .by_ref()
        .take(max_tar.saturating_add(1))
        .read_to_end(&mut buffer)
        .map_err(|error| format!("plugin sync archive exceeds limit: {error}"))?;
    if buffer.len() as u64 > max_tar {
        return Err(format!(
            "plugin sync archive exceeds limit: {} bytes",
            buffer.len()
        ));
    }
    let mut offset = 0usize;
    let mut extracted = 0u64;
    while offset + BLOCK <= buffer.len() {
        let header: &[u8; BLOCK] = buffer[offset..offset + BLOCK]
            .try_into()
            .map_err(|_| "truncated plugin sync archive entry".to_owned())?;
        offset += BLOCK;
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = archive_tar::parse_octal(&header[124..136])?;
        let data_start = offset;
        let data_end = data_start
            .checked_add(size as usize)
            .ok_or_else(|| "invalid plugin sync archive entry size".to_owned())?;
        if data_end > buffer.len() {
            return Err("truncated plugin sync archive entry".to_owned());
        }
        offset = data_start + archive_tar::padded(size)? as usize;
        let type_flag = {
            let value = archive_tar::read_string(&header[156..157]);
            if value.is_empty() {
                "0".to_owned()
            } else {
                value
            }
        };
        let archive_path = super::paths::normalize_relative(
            &archive_tar::entry_path(header),
            "unsafe plugin archive path",
        )?;
        let target =
            super::paths::resolve_within(target_dir, &archive_path, "unsafe plugin archive path")?;
        if type_flag == "5" {
            std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            continue;
        }
        if type_flag != "0" && type_flag != "\0" {
            return Err(format!(
                "unsupported plugin archive entry type: {type_flag}"
            ));
        }
        extracted = extracted
            .checked_add(size)
            .ok_or_else(|| "invalid plugin sync archive entry size".to_owned())?;
        if let Some(max) = max_extracted_bytes
            && extracted > max
        {
            return Err(format!(
                "plugin sync archive exceeds limit: {extracted}/{max}"
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let content = &buffer[data_start..data_end];
        write_file(&target, content, archive_tar::read_mode(header))?;
    }
    Ok(())
}

fn write_file(path: &Path, content: &[u8], mode: u64) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode as u32)
            .open(path)
            .map_err(|error| error.to_string())?;
        file.write_all(content).map_err(|error| error.to_string())?;
        file.set_permissions(std::fs::Permissions::from_mode(mode as u32))
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::write(path, content).map_err(|error| error.to_string())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
fn mtime_ms(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    u64::try_from(metadata.mtime())
        .unwrap_or(0)
        .saturating_mul(1000)
        + u64::from(metadata.mtime_nsec() as u32 / 1_000_000)
}

#[cfg(not(unix))]
fn mtime_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn mtime_seconds(metadata: &std::fs::Metadata) -> u64 {
    mtime_ms(metadata) / 1000
}

#[cfg(test)]
pub(super) fn test_header(
    path: &str,
    mode: u64,
    mtime_seconds: u64,
    size: u64,
    type_flag: u8,
) -> [u8; BLOCK] {
    archive_tar::header(path, mode, mtime_seconds, size, type_flag).expect("test header")
}
