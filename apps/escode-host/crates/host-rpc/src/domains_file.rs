//! `file` 域（rust-u2-host-skeleton.md §5 优先序 4）：精确子集。
//!
//! 已实现（逐字对齐 `packages/services/src/file/fileService.ts`）：
//! `stat` / `checkFilesExist` / `resolvePath` / `readFileRange` / `readTextFile` / `readdir`
//! （预览两法见 `domains_file_preview`，工作区创建三法见 `domains_file_workspace`）——这些方法
//! 只依赖绝对路径与 Host 路径配置，不需要 workspace 上下文与 runtime 实例。
//! `readdir` 的第二排序键是 `localeCompare`：用 ICU4X 复现 ICU 默认 collation，locale 按 Node
//! 的环境解析规则取（见 `collation.rs`），不是码点序近似。
//!
//! 未实现（各附原因，`--` 为运行时缺口可见）：
//! - `searchWorkspaceFiles` / `listWorkspaceFilesRange` / `workspaceFileListLength`：需 Host 侧
//!   文件索引（`.escodeignore` 指纹、60s TTL 缓存与分块打包）。

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::session::ChannelResult;
use crate::wire::Wire;

/// TS `FILE_EXISTENCE_BATCH_LIMIT`。
const FILE_EXISTENCE_BATCH_LIMIT: usize = 15;
/// TS `FILE_EXISTENCE_CACHE_TTL_MS`。
const FILE_EXISTENCE_CACHE_TTL: Duration = Duration::from_secs(60);
/// TS `FILE_EXISTENCE_CACHE_MAX_ENTRIES`。
const FILE_EXISTENCE_CACHE_MAX_ENTRIES: usize = 100;
/// TS `DEFAULT_BINARY_READ_BYTES` / `MAX_BINARY_READ_BYTES`。
const DEFAULT_BINARY_READ_BYTES: u64 = 256 * 1024;
const MAX_BINARY_READ_BYTES: u64 = 1024 * 1024;
/// TS `DEFAULT_TEXT_READ_BYTES` / `MAX_TEXT_READ_BYTES`。
const DEFAULT_TEXT_READ_BYTES: u64 = 128 * 1024;
const MAX_TEXT_READ_BYTES: u64 = 256 * 1024;

/// 文件系统 IO 放到阻塞线程（Host 的 runtime 工作线程不做同步 IO）。
pub(crate) async fn blocking(
    work: impl FnOnce() -> ChannelResult + Send + 'static,
) -> ChannelResult {
    spawn_blocking_io(work).await
}

/// 通用阻塞桥（`blocking` 的泛型版）：把同步 IO 放到阻塞线程池，panic 折叠为结构化错误。
pub(crate) async fn spawn_blocking_io<T>(work: impl FnOnce() -> T + Send + 'static) -> T
where
    T: FromBlockingFailure,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(T::from_blocking_failure)
}

/// 阻塞任务 panic 后的降级结果（每种返回类型各自决定）。
pub(crate) trait FromBlockingFailure: Send + 'static {
    fn from_blocking_failure(error: tokio::task::JoinError) -> Self;
}

impl<T> FromBlockingFailure for Result<T, (String, String)>
where
    T: Send + 'static,
{
    fn from_blocking_failure(error: tokio::task::JoinError) -> Self {
        Err(("Fatal".to_owned(), format!("blocking task failed: {error}")))
    }
}

/// TS `stat`：跟随符号链接；目录不返回 `size`/`mtimeMs`，文件返回两者（毫秒 f64）。
pub(crate) fn stat(params: &Value) -> ChannelResult {
    let path = require_path(params)?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "stat", &path)))?;
    let mut object = json!({
        "path": path,
        "type": if metadata.is_dir() { "directory" } else { "file" },
    });
    if !metadata.is_dir() {
        object["size"] = metadata.len().into();
        object["mtimeMs"] = mtime_ms(&metadata).into();
    }
    Ok(Wire::Object(object))
}

/// TS `checkFilesExist`：批上限 15；`exists` 取 `isFile()`（目录算 false）；错误按 false；保持入参顺序。
pub(crate) fn check_files_exist(cache: &std::sync::Mutex<Cache>, params: &Value) -> ChannelResult {
    let paths: Vec<String> = params["paths"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if paths.len() > FILE_EXISTENCE_BATCH_LIMIT {
        return Err((
            "InvalidRequest".to_owned(),
            format!("File existence check supports at most {FILE_EXISTENCE_BATCH_LIMIT} paths."),
        ));
    }
    let now = SystemTime::now();
    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        let cached = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&path, now);
        let exists = match cached {
            Some(exists) => exists,
            None => {
                // TS `checkFileExists`：`stat(path).isFile()`，失败按 false（正负结果都缓存）。
                let exists = std::fs::metadata(&path).is_ok_and(|meta| meta.is_file());
                cache
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set(path.clone(), exists, now);
                exists
            }
        };
        results.push(json!({"path": path, "exists": exists}));
    }
    Ok(Wire::Object(Value::Array(results)))
}

/// TS `resolvePath`：`fs.realpath`（符号链接别名归一，供 workspace 身份计算）。
pub(crate) async fn resolve_path(params: &Value) -> ChannelResult {
    let path = require_path(params)?;
    match escode_cli_host::realpath(&PathBuf::from(&path)).await {
        Ok(resolved) => Ok(Wire::Object(Value::String(
            resolved.to_string_lossy().into_owned(),
        ))),
        Err(_) => Err((
            "NodeIoError".to_owned(),
            format!("ENOENT: no such file or directory, realpath '{path}'"),
        )),
    }
}

/// TS `readTextFile`：文本切片（二进制探测命中时 `content` 为空串）。
/// 读取上限固定 256 KiB（缺省 128 KiB），`truncated` 由「offset + bytesRead < totalBytes」判定。
pub(crate) fn read_text_file(params: &Value) -> ChannelResult {
    let path = require_path(params)?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "stat", &path)))?;
    if !metadata.is_file() {
        return Err((
            "InvalidRequest".to_owned(),
            format!("Path is not a file: {path}"),
        ));
    }
    let size = metadata.len();
    let offset = params["offset"].as_f64().unwrap_or(0.0).max(0.0).trunc() as u64;
    if offset >= size {
        return Ok(Wire::Object(json!({
            "path": path,
            "content": "",
            "offset": offset,
            "bytesRead": 0,
            "totalBytes": size,
            "truncated": false,
            "isBinary": false,
        })));
    }
    let requested = params["length"]
        .as_f64()
        .filter(|value| value.is_finite())
        .map_or(DEFAULT_TEXT_READ_BYTES, |value| value.trunc() as u64);
    let length = requested.clamp(1, MAX_TEXT_READ_BYTES).min(size - offset);
    let chunk = read_at(&path, offset, length)?;
    let is_binary = is_probably_binary(&chunk);
    let bytes_read = chunk.len() as u64;
    Ok(Wire::Object(json!({
        "path": path,
        "content": if is_binary { String::new() } else { String::from_utf8_lossy(&chunk).into_owned() },
        "offset": offset,
        "bytesRead": bytes_read,
        "totalBytes": size,
        "truncated": offset + bytes_read < size,
        "isBinary": is_binary,
    })))
}

pub(crate) fn read_at(path: &str, offset: u64, length: u64) -> Result<Vec<u8>, (String, String)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "open", path)))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "read", path)))?;
    let mut buffer = vec![0_u8; length as usize];
    let mut filled = 0_usize;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => {
                return Err(("NodeIoError".to_owned(), node_error(&error, "read", path)));
            }
        }
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// TS `isProbablyBinary`：空缓冲 false；出现 NUL 立即 true；
/// 控制字符（1–8 / 14–31 / 127，常见空白 9/10/12/13 除外）占比 **> 0.3** 判二进制。
pub(crate) fn is_probably_binary(buffer: &[u8]) -> bool {
    if buffer.is_empty() {
        return false;
    }
    let mut suspicious = 0_usize;
    for value in buffer {
        let value = *value;
        if value == 0 {
            return true;
        }
        let common_whitespace = matches!(value, 9 | 10 | 12 | 13);
        let control = (1..=8).contains(&value) || (14..=31).contains(&value) || value == 127;
        if !common_whitespace && control {
            suspicious += 1;
        }
    }
    (suspicious as f64) / (buffer.len() as f64) > 0.3
}

/// TS `readFileRange`：非文件报错；`offset = max(0, trunc(offset))`；`offset ≥ size` → 空；
/// 长度 clamp 到 `[1, MAX_BINARY_READ_BYTES]`（缺省 256 KiB）；EOF 返回短数组；
/// 顶层 `Uint8Array`（`Wire::Bytes{vs:false}`，非 JSON+base64）。
pub(crate) fn read_file_range(params: &Value) -> ChannelResult {
    let path = require_path(params)?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "stat", &path)))?;
    if !metadata.is_file() {
        return Err((
            "InvalidRequest".to_owned(),
            format!("Path is not a file: {path}"),
        ));
    }
    let size = metadata.len();
    let offset = params["offset"].as_f64().unwrap_or(0.0).max(0.0).trunc() as u64;
    if offset >= size {
        return Ok(empty_bytes());
    }
    let requested = params["length"]
        .as_f64()
        .filter(|value| value.is_finite())
        .map_or(DEFAULT_BINARY_READ_BYTES, |value| value.trunc() as u64);
    let length = requested.clamp(1, MAX_BINARY_READ_BYTES).min(size - offset);
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "open", &path)))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "read", &path)))?;
    let mut buffer = vec![0_u8; length as usize];
    let mut filled = 0_usize;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => {
                return Err(("NodeIoError".to_owned(), node_error(&error, "read", &path)));
            }
        }
    }
    buffer.truncate(filled);
    Ok(Wire::Bytes {
        vs: false,
        data: buffer,
    })
}

pub(crate) fn empty_bytes() -> Wire {
    Wire::Bytes {
        vs: false,
        data: Vec::new(),
    }
}

pub(crate) fn require_path(params: &Value) -> Result<String, (String, String)> {
    params["path"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ("InvalidRequest".to_owned(), "path is required".to_owned()))
}

/// Node `fs.Stats.mtimeMs`：秒 + 纳秒/1e6 的 f64（含亚毫秒位）。
pub(crate) fn mtime_ms(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |since| {
            since.as_secs_f64() * 1000.0 + f64::from(since.subsec_nanos()) / 1_000_000.0
        })
}

/// Node 常见 errno 文案（`{CODE}: {description}, {op} '{path}'`）。
/// 未覆盖的 errno 保留系统文案（登记为已批准差异：客户端不解析该文案，仅展示）。
pub(crate) fn node_error(error: &std::io::Error, op: &str, path: &str) -> String {
    // `ErrorKind::{NotADirectory, IsADirectory, FilesystemLoop}` 在本工具链仍不稳定：
    // unix 用 raw errno 判（跨平台只覆盖 NXIO 之外的常见码），其余保留系统文案。
    let from_errno = match error.raw_os_error() {
        #[cfg(unix)]
        Some(20) => Some(("ENOTDIR", "not a directory")),
        #[cfg(unix)]
        Some(21) => Some(("EISDIR", "illegal operation on a directory")),
        #[cfg(unix)]
        Some(40) => Some(("ELOOP", "too many symbolic links encountered")),
        _ => None,
    };
    let (code, description) = match (error.kind(), from_errno) {
        (_, Some(pair)) => pair,
        (std::io::ErrorKind::NotFound, _) => ("ENOENT", "no such file or directory"),
        (std::io::ErrorKind::PermissionDenied, _) => ("EACCES", "permission denied"),
        (std::io::ErrorKind::AlreadyExists, _) => ("EEXIST", "file already exists"),
        _ => return error.to_string(),
    };
    format!("{code}: {description}, {op} '{path}'")
}

/// TS `FileExistenceCache`：60s TTL + 100 条上限；命中移到队尾（LRU），写入先清全部过期项。
#[derive(Default)]
pub(crate) struct Cache {
    entries: Vec<(String, Entry)>,
}

struct Entry {
    exists: bool,
    expires_at: SystemTime,
}

impl Cache {
    fn get(&mut self, path: &str, now: SystemTime) -> Option<bool> {
        let index = self.entries.iter().position(|(key, _)| key == path)?;
        if self.entries[index].1.expires_at <= now {
            self.entries.remove(index);
            return None;
        }
        let entry = self.entries.remove(index);
        let exists = entry.1.exists;
        self.entries.push(entry);
        Some(exists)
    }

    fn set(&mut self, path: String, exists: bool, now: SystemTime) {
        self.entries.retain(|(_, entry)| entry.expires_at > now);
        self.entries.retain(|(key, _)| key != &path);
        self.entries.push((
            path,
            Entry {
                exists,
                expires_at: now + FILE_EXISTENCE_CACHE_TTL,
            },
        ));
        while self.entries.len() > FILE_EXISTENCE_CACHE_MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}
#[cfg(test)]
#[path = "domains_file_tests.rs"]
mod tests;
