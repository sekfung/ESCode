//! Project Memory 目录视图（TS `memoryService.ts` 的 `listProjectMemories` / `readProjectMemoryFile`
//! 与 `projectMemoryStableRead.ts`）：只读、逐层拒绝符号链接、读文件走已验证的句柄。
//! docs/specs/rust-u2-user-config-services.md §2。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use escode_host_services::git::paths::js_trim;
use serde_json::{Value, json};

use crate::collation::compare_en;
use crate::domains_file::node_error;
use crate::session::RpcError;

const INDEX_FILE: &str = "MEMORY.md";
const MEMORY_DIR: &str = "memory";
const PREVIEW_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub(crate) const PREVIEW_LIMIT_CODE: &str = "PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED";
pub(crate) const FILE_CHANGED_CODE: &str = "PROJECT_MEMORY_FILE_CHANGED";

/// `<dataBaseDir>/.escode/cli/memories/projects`。
pub(crate) fn projects_root(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("cli")
        .join("memories")
        .join("projects")
}

fn node_io(error: &std::io::Error, op: &str, path: &Path) -> RpcError {
    let message = node_error(error, op, &path.to_string_lossy());
    // Node 的 fs 错误带 errno `code`（`ENOENT: …`），Renderer 据此分流。
    let code = message
        .split(':')
        .next()
        .filter(|code| {
            code.len() > 1 && code.starts_with('E') && code.chars().all(|c| c.is_ascii_uppercase())
        })
        .map(str::to_owned);
    let rpc = RpcError::new("NodeIoError", message);
    match code {
        Some(code) => rpc.with("code", Value::from(code)),
        None => rpc,
    }
}

fn not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

fn plain_error(message: String) -> RpcError {
    RpcError::new("Error", message)
}

/// TS `isProjectMemoryFileName`。
fn is_memory_file(name: &str) -> bool {
    name == INDEX_FILE || name.ends_with(".md")
}

/// TS `isValidPathSegment`（`basename(value) === value` 在 POSIX 上等价于不含分隔符）。
fn is_valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

/// TS `resolveWorkspaceLabel`：去掉 `-<16位十六进制>` 后缀。
fn workspace_label(id: &str) -> String {
    let stripped = id.len().checked_sub(17).and_then(|split| {
        let (head, tail) = id.split_at_checked(split)?;
        let hex = tail.strip_prefix('-')?;
        (hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(head)
    });
    match stripped.map(js_trim) {
        Some(label) if !label.is_empty() => label.to_owned(),
        _ => id.to_owned(),
    }
}

/// lstat：是真正的目录（不是符号链接）；不存在返回 `false`，其它错误上抛。
fn is_plain_dir(path: &Path) -> Result<bool, RpcError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.is_dir() && !meta.file_type().is_symlink()),
        Err(error) if not_found(&error) => Ok(false),
        Err(error) => Err(node_io(&error, "lstat", path)),
    }
}

fn require_plain_dir(path: &Path) -> Result<(), RpcError> {
    let meta = std::fs::symlink_metadata(path).map_err(|error| node_io(&error, "lstat", path))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(plain_error(format!(
            "Project Memory directory is not a regular directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn require_root(root: &Path) -> Result<(), RpcError> {
    require_plain_dir(root)
}

/// TS `requireExactProjectMemoryFile`：目录项名必须逐字相同（大小写不敏感文件系统不能别名命中）。
fn require_exact_file(memory_root: &Path, name: &str) -> Result<PathBuf, RpcError> {
    let entries =
        std::fs::read_dir(memory_root).map_err(|error| node_io(&error, "scandir", memory_root))?;
    let mut found = None;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy() == name {
            found = Some(entry);
            break;
        }
    }
    let requested = memory_root.join(name);
    let Some(entry) = found else {
        std::fs::symlink_metadata(&requested)
            .map_err(|error| node_io(&error, "lstat", &requested))?;
        return Err(plain_error(format!(
            "Project Memory file name does not match exactly: {name}"
        )));
    };
    let kind = entry
        .file_type()
        .map_err(|error| node_io(&error, "lstat", &requested))?;
    if !kind.is_file() || kind.is_symlink() {
        return Err(plain_error(format!(
            "Project Memory file is not a regular file: {name}"
        )));
    }
    Ok(requested)
}

/// TS `assertContainedProjectMemoryPath`。
fn assert_contained(projects_root: &Path, target: &Path) -> Result<(), RpcError> {
    let root_real =
        std::fs::canonicalize(projects_root).map_err(|e| node_io(&e, "realpath", projects_root))?;
    let target_real = std::fs::canonicalize(target).map_err(|e| node_io(&e, "realpath", target))?;
    if !target_real.starts_with(&root_real) {
        return Err(plain_error(format!(
            "Project Memory path is outside the local profile: {}",
            target.display()
        )));
    }
    Ok(())
}

/// libuv `mtimeMs`：`sec * 1000 + nsec / 1e6`。
fn mtime_ms(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| {
            d.as_secs() as f64 * 1000.0 + f64::from(d.subsec_nanos()) / 1e6
        })
}

/// TS `listProjectMemories`。
pub(crate) fn list(root: &Path) -> Result<Value, RpcError> {
    match require_root(root)
        .and_then(|()| std::fs::read_dir(root).map_err(|error| node_io(&error, "scandir", root)))
    {
        Err(error) if error.message.starts_with("ENOENT") => Ok(Value::Array(Vec::new())),
        Err(error) => Err(error),
        Ok(entries) => list_workspaces(root, entries),
    }
}

fn list_workspaces(root: &Path, entries: std::fs::ReadDir) -> Result<Value, RpcError> {
    let mut workspaces: Vec<(f64, String, Value)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let workspace_root = root.join(&id);
        let memory_root = workspace_root.join(MEMORY_DIR);
        if !is_plain_dir(&workspace_root)? || !is_plain_dir(&memory_root)? {
            continue;
        }
        let memory_entries = match std::fs::read_dir(&memory_root) {
            Ok(entries) => entries,
            Err(error) if not_found(&error) => continue,
            Err(error) => return Err(node_io(&error, "scandir", &memory_root)),
        };
        let mut files: Vec<(bool, String, f64, Value)> = Vec::new();
        for item in memory_entries.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let Ok(item_kind) = item.file_type() else {
                continue;
            };
            if !item_kind.is_file() || item_kind.is_symlink() || !is_memory_file(&name) {
                continue;
            }
            let path = memory_root.join(&name);
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if not_found(&error) => continue,
                Err(error) => return Err(node_io(&error, "lstat", &path)),
            };
            if !meta.is_file() || meta.file_type().is_symlink() {
                continue;
            }
            let updated = mtime_ms(&meta);
            let is_index = name == INDEX_FILE;
            files.push((
                is_index,
                name.clone(),
                updated,
                json!({
                    "name": name, "path": path.to_string_lossy(),
                    "kind": if is_index { "index" } else { "item" },
                    "size": meta.len(), "updatedAt": updated,
                }),
            ));
        }
        if files.is_empty() {
            continue;
        }
        // index 在前，其余按 `localeCompare(_, "en")`。
        files.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| compare_en(&left.1, &right.1))
        });
        let updated = files.iter().map(|file| file.2).fold(f64::MIN, f64::max);
        let summary = json!({
            "id": id, "label": workspace_label(&id), "updatedAt": updated,
            "files": files.into_iter().map(|file| file.3).collect::<Vec<_>>(),
        });
        workspaces.push((updated, id, summary));
    }
    workspaces.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| compare_en(&left.1, &right.1))
    });
    Ok(Value::Array(
        workspaces
            .into_iter()
            .map(|workspace| workspace.2)
            .collect(),
    ))
}

/// 文件身份快照（TS `isSameFileSnapshot`：dev/ino/size/mtimeNs/ctimeNs；Windows 没有 dev/ino/ctime）。
#[derive(PartialEq, Eq)]
struct Snapshot {
    #[cfg(unix)]
    ids: (u64, u64, i64, i64),
    size: u64,
    mtime_ns: i128,
}

fn snapshot(meta: &std::fs::Metadata) -> Snapshot {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Snapshot {
        #[cfg(unix)]
        ids: (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()),
        size: meta.len(),
        mtime_ns: meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| {
                i128::from(d.as_secs()) * 1_000_000_000 + i128::from(d.subsec_nanos())
            }),
    }
}

fn coded(message: String, code: &'static str) -> RpcError {
    RpcError::new("Error", message).with("code", Value::from(code))
}

/// TS `readProjectMemoryFile` + `readProjectMemoryFileFromStableHandle`。
pub(crate) fn read_file(
    root: &Path,
    workspace_id: &str,
    file_name: &str,
) -> Result<Value, RpcError> {
    use std::io::Read;
    if !is_valid_segment(workspace_id) || !is_valid_segment(file_name) || !is_memory_file(file_name)
    {
        return Err(plain_error("Invalid Project Memory path".to_owned()));
    }
    require_root(root)?;
    let workspace_root = root.join(workspace_id);
    let memory_root = workspace_root.join(MEMORY_DIR);
    require_plain_dir(&workspace_root)?;
    require_plain_dir(&memory_root)?;
    let path = require_exact_file(&memory_root, file_name)?;

    let pre = std::fs::symlink_metadata(&path).map_err(|e| node_io(&e, "lstat", &path))?;
    if !pre.is_file() || pre.file_type().is_symlink() {
        return Err(plain_error(format!(
            "Project Memory file is not a regular file: {}",
            path.display()
        )));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut handle = options
        .open(&path)
        .map_err(|e| node_io(&e, "open", &path))?;
    let opened = handle.metadata().map_err(|e| node_io(&e, "fstat", &path))?;
    // 路径检查后再次确认路径仍指向同一受信任文件。
    require_root(root)?;
    require_plain_dir(&workspace_root)?;
    require_plain_dir(&memory_root)?;
    require_exact_file(&memory_root, file_name)?;
    assert_contained(root, &path)?;
    let post = std::fs::symlink_metadata(&path).map_err(|e| node_io(&e, "lstat", &path))?;
    let changed = || {
        coded(
            format!("Project Memory file changed during preview: {file_name}"),
            FILE_CHANGED_CODE,
        )
    };
    let too_large = || {
        coded(
            format!("Project Memory file exceeds the 5 MiB preview limit: {file_name}"),
            PREVIEW_LIMIT_CODE,
        )
    };
    if !opened.is_file()
        || !post.is_file()
        || post.file_type().is_symlink()
        || snapshot(&opened) != snapshot(&pre)
        || snapshot(&opened) != snapshot(&post)
    {
        return Err(changed());
    }
    if opened.len() > PREVIEW_MAX_BYTES {
        return Err(too_large());
    }
    let mut content = Vec::new();
    (&mut handle)
        .take(PREVIEW_MAX_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|e| node_io(&e, "read", &path))?;
    let last = handle.metadata().map_err(|e| node_io(&e, "fstat", &path))?;
    if snapshot(&opened) != snapshot(&last) {
        return Err(changed());
    }
    if content.len() as u64 > PREVIEW_MAX_BYTES || last.len() > PREVIEW_MAX_BYTES {
        return Err(too_large());
    }
    Ok(json!({
        "content": String::from_utf8_lossy(&content),
        "updatedAt": snapshot(&last).mtime_ns as f64 / 1_000_000.0,
    }))
}

#[cfg(test)]
#[path = "project_memory_tests.rs"]
mod tests;
