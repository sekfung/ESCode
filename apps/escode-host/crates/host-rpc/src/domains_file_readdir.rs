//! `file.readdir`：lstat 语义 + 隐藏项过滤 + 「目录组优先、组内 localeCompare」排序。
//! 排序用 `collation.rs` 的 ICU collation（Node `localeCompare` 等价），非码点序近似。

use serde_json::{Value, json};

use crate::domains_file::{node_error, require_path};
use crate::session::ChannelResult;
use crate::wire::Wire;

/// TS `readdir`：`withFileTypes` 的 lstat 语义 + 隐藏项过滤 + 「目录组优先、组内 localeCompare」排序。
///
/// 类型判定（`resolveReaddirEntryType` 同序）：目录 → `directory`；非软链 → `file`；
/// 软链 → 跟随目标 stat（目录 → `directory`，stat 失败 → `file`）。
pub(crate) fn readdir(params: &Value) -> ChannelResult {
    let dir = require_path(params)?;
    let include_hidden = params["includeHidden"].as_bool().unwrap_or(false);
    let entries = std::fs::read_dir(&dir).map_err(|error| {
        (
            "NodeIoError".to_owned(),
            node_error(&error, "scandir", &dir),
        )
    })?;
    let mut rows: Vec<(String, String, String, bool)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            (
                "NodeIoError".to_owned(),
                node_error(&error, "scandir", &dir),
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !include_hidden && name.starts_with('.') {
            continue;
        }
        let is_symlink = entry
            .file_type()
            .map(|kind| kind.is_symlink())
            .unwrap_or(false);
        let is_directory = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let kind = if is_directory {
            "directory"
        } else if !is_symlink {
            "file"
        } else {
            // 跟随软链：目录 → directory，其余（含 stat 失败）→ file。
            match std::fs::metadata(entry.path()) {
                Ok(metadata) if metadata.is_dir() => "directory",
                _ => "file",
            }
        };
        let path = node_join(&dir, &name);
        rows.push((name, path, kind.to_owned(), is_symlink));
    }
    // 稳定排序：类型组（directory 在前）+ 组内 localeCompare；同序对保留 readdir 原始顺序。
    rows.sort_by(|left, right| {
        if left.2 != right.2 {
            return if left.2 == "directory" {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        crate::collation::compare_names(&left.0, &right.0)
    });
    let rows: Vec<Wire> = rows
        .into_iter()
        .map(|(name, path, kind, is_symlink)| {
            Wire::Object(json!({
                "name": name,
                "path": path,
                "type": kind,
                "isSymbolicLink": is_symlink,
            }))
        })
        .collect();
    Ok(Wire::Array(rows))
}

/// Node `path.join(base, name)`：拼接后归一化（折叠空段与 `.`、消解 `..`、去尾斜杠）。
/// readdir 的 entry 路径直接对外可见，未归一化会与 TS 端同输入的输出不一致。
pub(crate) fn node_join(base: &str, name: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    let mut rooted = false;
    let mut prefix = "";
    for (index, segment) in base.split(['/', '\\']).chain([name]).enumerate() {
        if index == 0 && segment.is_empty() {
            // POSIX 绝对路径（`/a` 的首段为空）。
            rooted = true;
            continue;
        }
        if index == 0 && segment.len() == 2 && segment.ends_with(':') {
            // Windows 盘符（`C:\a`）。
            rooted = true;
            prefix = segment;
            continue;
        }
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    let joined = segments.join(std::path::MAIN_SEPARATOR_STR);
    if prefix.is_empty() {
        if rooted {
            format!("{}{joined}", std::path::MAIN_SEPARATOR_STR)
        } else {
            joined
        }
    } else {
        format!("{prefix}{}{joined}", std::path::MAIN_SEPARATOR_STR)
    }
}

#[cfg(test)]
#[path = "domains_file_readdir_tests.rs"]
mod tests;
