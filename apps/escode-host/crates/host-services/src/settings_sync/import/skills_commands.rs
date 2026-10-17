//! skills / commands 的导入（TS `importSkillsForAgent` / `importCommandsForAgent`）。

use std::path::{Path, PathBuf};

use super::{ImportMode, Selection, TaskResult, agent_matches, command_item, skill_item};
use crate::skills::Compare;

use super::super::candidates::{collect_command_candidates, collect_skill_candidates, target_root};
use super::super::discovery::skip::{SkipCaches, command_skip, skill_skip};

pub fn import_skills(
    home: &Path,
    workspace: Option<&Path>,
    selection: &Selection,
    compare: &Compare,
) -> TaskResult {
    let candidates = collect_skill_candidates(home, workspace, compare);
    let mut task = TaskResult::new(&selection.agent, "skills", "skillResults");
    let mut caches = SkipCaches::default();
    for candidate in candidates
        .iter()
        .filter(|candidate| agent_matches(candidate.agent, selection))
        .filter(|candidate| selection.source_scope_matches(candidate.source_root_scope))
        .filter(|candidate| selection.selected(&candidate.source_path.to_string_lossy()))
    {
        let selected_root = match selection.target_scope {
            Some(scope) => target_root(home, workspace, "skills", scope),
            None => Some(candidate.target_root.clone()),
        };
        let Some(selected_root) = selected_root else {
            task.skipped_item(skill_item(candidate), "targetExists");
            continue;
        };
        let target_path = selected_root.join(
            candidate
                .source_path
                .file_name()
                .map(std::ffi::OsStr::to_os_string)
                .unwrap_or_default(),
        );
        if let Some(reason) = skill_skip(
            &selected_root,
            &target_path,
            &candidate.name_key,
            &mut caches,
        ) {
            task.skipped_item(skill_item(candidate), reason);
            continue;
        }
        let result = (|| -> Result<(), ()> {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| ())?;
            }
            import_directory(&candidate.source_path, &target_path, selection.mode)
        })();
        match result {
            Ok(()) => {
                caches.remember_skill(&selected_root, &candidate.name_key);
                task.imported_item(skill_item(candidate));
            }
            Err(()) => task.failed_item(skill_item(candidate)),
        }
    }
    task
}

pub fn import_commands(
    home: &Path,
    workspace: Option<&Path>,
    selection: &Selection,
    compare: &Compare,
) -> TaskResult {
    let candidates = collect_command_candidates(home, workspace, compare);
    let mut task = TaskResult::new(&selection.agent, "commands", "commandResults");
    let mut caches = SkipCaches::default();
    for candidate in candidates
        .iter()
        .filter(|candidate| agent_matches(candidate.agent, selection))
        .filter(|candidate| selection.source_scope_matches(candidate.source_root_scope))
        .filter(|candidate| selection.selected(&candidate.source_path.to_string_lossy()))
    {
        let selected_root = match selection.target_scope {
            Some(scope) => target_root(home, workspace, "commands", scope),
            None => Some(candidate.target_root.clone()),
        };
        let Some(selected_root) = selected_root else {
            task.skipped_item(command_item(candidate), "targetExists");
            continue;
        };
        let target_path = join_relative(&selected_root, &candidate.source_relative_path);
        if let Some(reason) = command_skip(
            &selected_root,
            &target_path,
            &candidate.name_key,
            &mut caches,
        ) {
            task.skipped_item(command_item(candidate), reason);
            continue;
        }
        let result = (|| -> Result<(), ()> {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| ())?;
            }
            import_file(&candidate.source_path, &target_path, selection.mode)
        })();
        match result {
            Ok(()) => {
                caches.remember_command(&selected_root, &candidate.name_key);
                task.imported_item(command_item(candidate));
            }
            Err(()) => task.failed_item(command_item(candidate)),
        }
    }
    task
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in relative.split('/').filter(|segment| !segment.is_empty()) {
        path.push(segment);
    }
    path
}

/// TS `importSkillDirectory` / `importPluginDirectory`。
pub(super) fn import_directory(source: &Path, target: &Path, mode: ImportMode) -> Result<(), ()> {
    if mode == ImportMode::Symlink {
        return create_directory_symlink(source, target);
    }
    copy_tree(source, target)
}

/// TS `importCommandFile`：Unix 文件软链；Windows 先硬链、失败回退复制。
pub(super) fn import_file(source: &Path, target: &Path, mode: ImportMode) -> Result<(), ()> {
    if mode == ImportMode::Symlink {
        #[cfg(unix)]
        {
            let source = absolute(source)?;
            return std::os::unix::fs::symlink(source, target).map_err(|_| ());
        }
        #[cfg(not(unix))]
        {
            let source = absolute(source)?;
            if std::fs::hard_link(&source, target).is_ok() {
                return Ok(());
            }
        }
    }
    copy_file(source, target)
}

fn create_directory_symlink(source: &Path, target: &Path) -> Result<(), ()> {
    let source = absolute(source)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target).map_err(|_| ())
    }
    #[cfg(windows)]
    {
        // TS 在 Windows 用 junction（免提权）；std 没有 junction API，这里用目录软链（可能需要开发者模式）。
        std::os::windows::fs::symlink_dir(source, target).map_err(|_| ())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(())
    }
}

fn absolute(path: &Path) -> Result<PathBuf, ()> {
    std::path::absolute(path).map_err(|_| ())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), ()> {
    let metadata = std::fs::symlink_metadata(source).map_err(|_| ())?;
    if metadata.is_dir() {
        std::fs::create_dir_all(target).map_err(|_| ())?;
        for entry in std::fs::read_dir(source).map_err(|_| ())? {
            let entry = entry.map_err(|_| ())?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        {
            let link = std::fs::read_link(source).map_err(|_| ())?;
            return std::os::unix::fs::symlink(link, target).map_err(|_| ());
        }
    }
    copy_file(source, target)
}

fn copy_file(source: &Path, target: &Path) -> Result<(), ()> {
    let mut input = std::fs::File::open(source).map_err(|_| ())?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|_| ())?;
    std::io::copy(&mut input, &mut output).map_err(|_| ())?;
    Ok(())
}
