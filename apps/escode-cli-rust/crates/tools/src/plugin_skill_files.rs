//! SKILL.md 文件扫描单点（rust-unified-host.md P1 #9「SKILL.md 三处扫描合并单点」）。
//!
//! 逐字对齐 TS `scanSkillFilesUnderRootSync(root, { followSymbolicLinks: false })`
//! （apps/escode-cli/packages/adapters/src/skills/scan.ts）：
//!
//! 1. 根是符号链接 → 空；
//! 2. 根不是目录 → 空；
//! 3. 根自身的 `SKILL.md` 若是**真实文件**（非符号链接）→ 收录；
//! 4. 一级子目录（`should_walk` 过滤）下的 `SKILL.md` 同理收录；
//! 5. **目录项顺序即 Node `readdirSync` 顺序**（TS 不另排序；但 libuv 在 Unix 上按名字字节序排序，
//!    Rust `read_dir` 是文件系统原始顺序，所以经 `plugin_describe_scan::entries` 对齐）。
//!
//! 不跟随符号链接是文件级逃逸防线（SKILL.md -> 外部任意文件），三处调用方共用本实现。

use std::path::{Path, PathBuf};

pub(super) const SKILL_FILE_NAME: &str = "SKILL.md";

/// TS `shouldWalkSkillDirectoryEntry`：跳过隐藏目录（`.system` 除外）与构建/依赖目录。
pub(super) fn should_walk(name: &str) -> bool {
    if name.starts_with('.') && name != ".system" {
        return false;
    }
    ![
        "node_modules",
        "dist",
        "build",
        "out",
        "target",
        "coverage",
        ".git",
    ]
    .contains(&name)
}

/// 同步扫描（调用方在阻塞线程或纯同步上下文里使用）。
pub(super) fn scan_skill_files_under_sync(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if std::fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return files;
    }
    if !std::fs::metadata(root).is_ok_and(|meta| meta.is_dir()) {
        return files;
    }
    let own = root.join(SKILL_FILE_NAME);
    if is_real_file(&own) {
        files.push(own);
    }
    // 修复：此前直接按 `read_dir` 原始顺序（ext4 为哈希序）遍历，而 Node `readdirSync` 在 Unix 上经 libuv
    // scandir 按名字字节序排序，plugins/list 的技能顺序因此与 Node 分叉。改用与 plugin_describe 同一份目录项读取。
    for (name, kind) in super::plugin_describe_scan::entries(root) {
        if !kind.is_dir() {
            continue;
        }
        if !should_walk(&name) {
            continue;
        }
        let candidate = root.join(&name).join(SKILL_FILE_NAME);
        if is_real_file(&candidate) {
            files.push(candidate);
        }
    }
    files
}

/// 异步包装：同步扫描放到受管阻塞线程（不占 runtime 工作线程）。
pub(super) async fn scan_skill_files_under(root: &Path) -> Vec<PathBuf> {
    let root = root.to_path_buf();
    escode_cli_core_api::scope::Spawner::current()
        .spawn_blocking("skill-file-scan", move || {
            scan_skill_files_under_sync(&root)
        })
        .await
        .unwrap_or_default()
}

/// 真实文件判定：符号链接一律拒绝（含 junction），目录不算。
fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_root_and_one_level_without_following_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join(SKILL_FILE_NAME), "# own\n").unwrap();
        std::fs::write(root.join("alpha").join(SKILL_FILE_NAME), "# alpha\n").unwrap();
        std::fs::write(root.join("beta").join(SKILL_FILE_NAME), "# beta\n").unwrap();
        std::fs::write(root.join(".hidden").join(SKILL_FILE_NAME), "# hidden\n").unwrap();
        std::fs::write(root.join("node_modules").join(SKILL_FILE_NAME), "# deps\n").unwrap();

        let files = scan_skill_files_under_sync(&root);
        let names: Vec<String> = files
            .iter()
            .map(|file| {
                file.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names[0], SKILL_FILE_NAME, "根自身先收录（TS 同序）");
        assert!(names.contains(&format!("alpha/{SKILL_FILE_NAME}")));
        assert!(names.contains(&format!("beta/{SKILL_FILE_NAME}")));
        assert_eq!(names.len(), 3, "隐藏目录与 node_modules 被跳过: {names:?}");
    }

    #[test]
    fn subdirectories_follow_node_readdir_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        // 故意按非字典序创建：Unix 上 Node readdirSync 按字节序返回，与创建顺序和 ext4 哈希序无关。
        let created = [
            "web-gui-tester",
            "diagnosing-skills",
            "control-browser",
            "Zeta",
        ];
        for name in created {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join(SKILL_FILE_NAME), "# s\n").unwrap();
        }
        let names: Vec<String> = scan_skill_files_under_sync(&root)
            .iter()
            .map(|file| {
                let parent = file.parent().unwrap().file_name().unwrap();
                parent.to_string_lossy().into_owned()
            })
            .collect();
        if cfg!(windows) {
            assert_eq!(names.len(), created.len());
        } else {
            assert_eq!(
                names,
                [
                    "Zeta",
                    "control-browser",
                    "diagnosing-skills",
                    "web-gui-tester"
                ]
            );
        }
    }

    #[test]
    fn refuses_symlinked_root_and_symlinked_skill_files() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join(SKILL_FILE_NAME), "# outside\n").unwrap();
        let root = dir.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        // 子目录里的 SKILL.md 是指向外部文件的符号链接：必须拒绝（逃逸防线）。
        std::fs::create_dir_all(root.join("evil")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.join(SKILL_FILE_NAME),
            root.join("evil").join(SKILL_FILE_NAME),
        )
        .unwrap();
        // 根本身是符号链接：直接返回空。
        #[cfg(unix)]
        let linked_root = {
            let link = dir.path().join("linked");
            std::os::unix::fs::symlink(&root, &link).unwrap();
            link
        };
        let files = scan_skill_files_under_sync(&root);
        #[cfg(unix)]
        assert!(files.is_empty(), "符号链接文件与目录都不收录: {files:?}");
        #[cfg(unix)]
        assert!(scan_skill_files_under_sync(&linked_root).is_empty());
        #[cfg(not(unix))]
        let _ = files;
    }
}
