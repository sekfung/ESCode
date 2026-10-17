//! `gen-ui` 的路径解析（TS `packages/shared/src/node/genUiPaths.ts` + stateStorage 的目录派生）。
//!
//! 两个 hash 都是**完整 64 位十六进制**（不截断，与 git-checkpoint 的 12 位口径无关）。
//! scope key 是 hash 输入，故其序列化必须与 JS 逐字一致（见 `schema::scope_key`）。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::schema::{Scope, Target, scope_key};

fn hex_sha256(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// `<dataBaseDir>/.escode/v2/gen-ui-state`（TS `getAppConfigDir()/gen-ui-state`）。
pub fn state_root(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("gen-ui-state")
}

/// `<dataBaseDir>/.escode/v2/visualizations`（TS `getAppConfigDir()/GEN_UI_OUTPUT_DIRECTORY`；
/// Main spawn agent 时经 `ESCODE_GEN_UI_OUTPUT_ROOT` 注入同一值）。
pub fn output_root(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("visualizations")
}

/// `<状态根>/<sha256(scopeKey)>`。
pub fn scope_dir(root: &Path, scope: &Scope) -> PathBuf {
    root.join(hex_sha256(&scope_key(scope)))
}

/// `<scope 目录>/<sha256(target.path)>.json`。
pub fn record_file(root: &Path, target: &Target) -> PathBuf {
    scope_dir(root, &target.scope).join(format!("{}.json", hex_sha256(&target.path)))
}

/// 状态文件名口径：`^[a-f0-9]{64}\.json$`（watcher 过滤与 `listState` 共用）。
pub fn is_record_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    stem.len() == 64
        && stem
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// TS/watcher 过滤用的**状态文件路径**口径：目录里的文件名必须合法才认。
pub fn record_file_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    is_record_file_name(name).then(|| dir.join(name))
}

/// 路径分隔符口径：Windows 上 `\` 与 `/` 都是分隔符，其余平台只有 `/`
/// （POSIX 下 `\` 是合法文件名字符，不能当分隔符——与 `node:path` 的 posix 实现一致）。
pub fn is_separator(character: char) -> bool {
    character == '/' || (cfg!(windows) && character == '\\')
}

/// `node:path.normalize` 的词法归一：折叠重复分隔符与 `.` 段。
///
/// **不处理 `..`**：调用方在此之前已过 `gen-ui` 的路径 schema（它拒绝任何 `..` 段），
/// 因此这里只需处理 `.` 与重复分隔符——`..` 的语义化解不在本函数职责内。
pub fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let absolute = path.starts_with(is_separator);
    let segments: Vec<&str> = path
        .split(is_separator)
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    let joined = segments.join(std::path::MAIN_SEPARATOR_STR);
    if absolute {
        format!("{}{joined}", std::path::MAIN_SEPARATOR)
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

/// TS `path.relative(root, path)` 在本域的用法只有一种判据：
/// **`path` 是否严格位于 `root` 之下**。是则回相对段，否则 `None`
/// （空相对、`..` 开头、绝对路径三种情况在 TS 侧同判为"越界"，这里合并成一个 `None`）。
pub fn child_of(root: &str, path: &str) -> Option<String> {
    if path == root {
        return None;
    }
    let mut prefix = root.to_owned();
    if !prefix.ends_with(is_separator) {
        prefix.push(std::path::MAIN_SEPARATOR);
    }
    path.strip_prefix(&prefix).map(str::to_owned)
}

/// 记录按 `updatedAt` 降序排序（同值保持发现序，与 TS 的 `Array.prototype.sort` 稳定序一致）。
pub fn sort_by_updated_desc<T>(items: &mut [T], updated_at: impl Fn(&T) -> f64) {
    items.sort_by(|left, right| {
        updated_at(right)
            .partial_cmp(&updated_at(left))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
