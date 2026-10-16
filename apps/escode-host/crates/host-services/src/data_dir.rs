//! 数据根目录迁移与默认项目（TS `settingService.updateDataBaseDir` / `ensureDefaultProject` + `paths.ts`）。
//! docs/specs/rust-u2-user-config-services.md §4。

use std::path::{Path, PathBuf};

use crate::git::paths::js_trim;

/// TS `DEFAULT_PROJECT_NAME`。
pub const DEFAULT_PROJECT_NAME: &str = "ESCodeProject";
/// TS `DATA_BASE_DIR_FORBIDDEN_WINDOWS_INSTALL_DIR_ERROR_CODE`。
pub const FORBIDDEN_INSTALL_DIR_CODE: &str = "DATA_BASE_DIR_FORBIDDEN_WINDOWS_INSTALL_DIR";

/// TS `ensureDefaultProject(userHomeDir)`：`{path, created}`。
pub async fn ensure_default_project(user_home: &Path) -> std::io::Result<(PathBuf, bool)> {
    let path = user_home.join(DEFAULT_PROJECT_NAME);
    let existed = tokio::fs::metadata(&path).await.is_ok();
    tokio::fs::create_dir_all(&path).await?;
    Ok((path, !existed))
}

/// Windows 路径的比较形式（TS `normalizeWindowsComparablePath`：规范化分隔符与 `.`/`..`、去尾部分隔符、小写）。
fn comparable(value: &str) -> Option<String> {
    let trimmed = js_trim(value);
    if trimmed.is_empty() {
        return None;
    }
    let unified = trimmed.replace('/', "\\");
    let (prefix, rest) = match unified.split_once(':') {
        Some((drive, rest)) if drive.len() == 1 => (format!("{drive}:"), rest.to_owned()),
        _ => (String::new(), unified.clone()),
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in rest.split('\\') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let joined = format!("{prefix}\\{}", parts.join("\\"));
    let normalized = joined.trim_end_matches('\\');
    (!normalized.is_empty()).then(|| normalized.to_lowercase())
}

fn equal_or_inside(path: &str, root: &str) -> bool {
    match (comparable(path), comparable(root)) {
        (Some(path), Some(root)) => path == root || path.starts_with(&format!("{root}\\")),
        _ => false,
    }
}

/// TS `validateDataBaseDirTarget`：只在 Windows 上拒绝落入应用安装目录；`lookup` 读环境变量（大小写不敏感）。
/// 返回被命中的禁用目录（原文）。
pub fn forbidden_install_dir(
    target: &str,
    is_windows: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if !is_windows {
        return None;
    }
    let env = |key: &str| {
        lookup(key)
            .map(|v| js_trim(&v).to_owned())
            .filter(|v| !v.is_empty())
    };
    let join = |base: Option<String>, tail: &str| {
        base.map(|b| format!("{}\\{tail}", b.trim_end_matches('\\')))
    };
    let candidates = [
        env("ESCODE_WINDOWS_APP_INSTALL_DIR"),
        join(env("ProgramFiles"), "ESCode"),
        join(env("ProgramFiles(x86)"), "ESCode"),
        join(env("ProgramW6432"), "ESCode"),
        env("LOCALAPPDATA").map(|b| format!("{}\\Programs\\ESCode", b.trim_end_matches('\\'))),
    ];
    let mut seen = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        .flatten()
        .filter(|candidate| comparable(candidate).is_some_and(|key| seen.insert(key)))
        .find(|candidate| equal_or_inside(target, candidate))
}

/// TS `copyDataDirectory`：`<old>/.escode/v2` → `<new>/.escode/v2`，不覆盖已有文件，
/// 跳过 `setting.json*`（bootstrap 状态只放默认家目录）与符号链接。
pub fn copy_data_directory(old_base: &Path, new_base: &Path) -> std::io::Result<()> {
    let source = old_base.join(".escode").join("v2");
    let target = new_base.join(".escode").join("v2");
    copy_tree(&source, &target)
}

fn skipped(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name == "setting.json" || name.starts_with("setting.json.")
}

fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    if skipped(source) {
        return Ok(());
    }
    // lstat 失败时放行，交给后续操作报错（TS 同）。
    let meta = std::fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !target.exists() {
        std::fs::copy(source, target)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "data_dir_tests.rs"]
mod tests;
