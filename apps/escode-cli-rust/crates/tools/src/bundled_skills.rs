//! 随 CLI 内置的技能包（TS bootstrap/app/bundled-skills.ts，docs/specs/rust-dynamic-workflow.md）。
//!
//! 它不是插件：不进官方市场、没有启停开关，也不出现在 `$` 引用面板（`SkillCatalog::response` 按
//! `system` scope 排除）。技能目录里排在所有插件根之后，同名时用户 / 项目 / 插件技能优先。
//! 动态工作流关闭的会话在固化技能目录时去掉它（core `freeze_skills`），与 TS `disabledPaths` 同义。
//!
//! 定位与 TS `resolveFilesystemBundledSkillPackRoot` 相同：沿官方插件同款候选目录（显式基准目录、
//! Node 入口所在目录、二进制所在目录、cwd）找 `bundled-skills`，三个必需文件缺任何一个都拒绝整包。

use std::path::{Path, PathBuf};

/// 技能门（SaveWorkflow 等写脚本的工具）要求会话里加载过的技能名。
pub(crate) const DYNAMIC_WORKFLOW_SKILL: &str = "dynamic-workflows";
/// 内置技能包在技能目录里的 scope（TS `scope: "system"`, `source: "bundled"`）。
pub(crate) const SCOPE: &str = "system";

const REQUIRED: [&str; 3] = [
    "skills/dynamic-workflows/SKILL.md",
    "skills/dynamic-workflows/patterns.md",
    "skills/dynamic-workflows/examples.md",
];
const ROOT_CANDIDATES: [&str; 4] = [
    "packages/bundled-skills",
    "../bundled-skills",
    "../../bundled-skills",
    "../../../bundled-skills",
];

fn base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("ESCODE_OFFICIAL_PLUGINS_BASE_DIR")
        && !dir.is_empty()
    {
        dirs.push(PathBuf::from(dir));
    }
    // Host 给插件宿主的 Node 入口（escode.cjs）所在目录：桌面与远端都把技能包 stage 在它旁边。
    if let Ok(entry) = std::env::var("ESCODE_PLUGIN_HOST_ENTRYPOINT")
        && let Some(dir) = Path::new(&entry).parent()
    {
        dirs.push(dir.to_owned());
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
    {
        dirs.push(dir);
    }
    if let Ok(dir) = std::env::current_dir() {
        dirs.push(dir);
    }
    dirs
}

/// 内置技能包的 `skills` 目录；缺席或不完整时为 None（写脚本的工具届时被技能门拒绝）。
pub(crate) fn skills_root() -> Option<PathBuf> {
    base_dirs().into_iter().find_map(|base| {
        ROOT_CANDIDATES.iter().find_map(|relative| {
            let pack = super::lexical_path::normalize(&base.join(relative));
            let complete = pack.join("skills").is_dir()
                && REQUIRED.iter().all(|file| pack.join(file).is_file());
            complete.then(|| pack.join("skills"))
        })
    })
}
