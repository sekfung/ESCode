//! 已保存工作流从全局移动到项目（TS `moveSavedWorkflowToProject`）与存储层的 IO 辅助。
#[allow(unused_imports)]
use super::saved_workflows::*;

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[allow(unused_imports)]
pub use super::saved_workflows_hub::{hub_delete, hub_get, hub_list, hub_move, hub_update_meta};
#[allow(unused_imports)]
pub use super::saved_workflows_model::{
    display, entry_json, format_model_content, model_content, output, to_value,
};

pub enum Move {
    Ok { from: PathBuf, to: PathBuf },
    InvalidName { detail: String },
    NotFound,
    TargetExists { path: PathBuf },
    ReadError { path: PathBuf, detail: String },
    WriteError { path: PathBuf, detail: String },
}

/// 把全局档的 `name` **逐字节**搬到 `cwd` 的项目档（只有这一向）。
///
/// 反向（项目→全局）不是搬文件：项目档大多引用本仓库的路径与命令，逐字节搬过去就是一个在别的
/// 项目里必然跑坏的全局定义。目标已存在即拒绝（覆盖只有 SaveWorkflow 经确认窗才有）。
pub fn move_to_project(cwd: &Path, home: &Path, name: &str) -> Move {
    if !is_valid_name(name) {
        return Move::InvalidName {
            detail: INVALID_NAME_DETAIL.to_owned(),
        };
    }
    let from_root = root(cwd, Scope::Global, home);
    let to_root = root(cwd, Scope::Project, home);
    let from = saved_path(&from_root, name);
    let to = saved_path(&to_root, name);
    if !file_exists(&from) {
        return Move::NotFound;
    }
    if file_exists(&to) {
        return Move::TargetExists { path: to };
    }
    if let Err(error) = std::fs::create_dir_all(&to_root.dir) {
        return Move::WriteError {
            path: to,
            detail: describe(&error),
        };
    }
    match std::fs::rename(&from, &to) {
        Ok(()) => return Move::Ok { from, to },
        // 跨设备时 rename 报 EXDEV：读→写→删的回落搬运，读写各自归错。
        Err(error) if !is_cross_device(&error) => {
            return Move::WriteError {
                path: to,
                detail: describe(&error),
            };
        }
        Err(_) => {}
    }
    let bytes = match std::fs::read(&from) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Move::ReadError {
                path: from,
                detail: describe(&error),
            };
        }
    };
    if let Err(error) = std::fs::write(&to, bytes) {
        return Move::WriteError {
            path: to,
            detail: describe(&error),
        };
    }
    if let Err(error) = std::fs::remove_file(&from) {
        return Move::WriteError {
            path: to,
            detail: describe(&error),
        };
    }
    Move::Ok { from, to }
}

pub fn file_exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// TS 把 `ENOENT` 与 `ENOTDIR`（`.zcode/workflows` 被人建成了文件）都当作"没找到"。
pub fn is_not_found(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
}

#[cfg(unix)]
pub fn is_cross_device(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::EXDEV)
}

#[cfg(windows)]
pub fn is_cross_device(error: &std::io::Error) -> bool {
    // MoveFileEx 的 ERROR_NOT_SAME_DEVICE（Node 的 libuv 用 COPY_ALLOWED 自己处理，std 不处理）。
    error.raw_os_error() == Some(17)
}

pub fn describe(error: &std::io::Error) -> String {
    error.to_string()
}

/// JS 默认排序（按 UTF-16 码元），与 `String.prototype.localeCompare` 无关。
pub fn js_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}
