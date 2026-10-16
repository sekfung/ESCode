//! 技能扫描策略与默认启用的官方插件（TS `@escode/shared` 的 `skill-scan-policy.ts` / `plugin-marketplaces.ts`）。
//! 与 TS 的一致性由 `policy_golden.json` 守住（`scripts/generate-escode-host-skills-fixtures.mjs --check`）。

pub const SKILL_FILE_NAME: &str = "SKILL.md";
pub const MAX_SCAN_DEPTH: usize = 8;

const EXCLUDED_DIRECTORIES: [&str; 12] = [
    "node_modules",
    "dist",
    "build",
    "out",
    "target",
    "vendor",
    "coverage",
    ".cache",
    ".next",
    ".turbo",
    ".venv",
    "__pycache__",
];

/// TS `shouldWalkSkillDirectoryEntry`。
pub fn should_walk(entry_name: &str) -> bool {
    if EXCLUDED_DIRECTORIES.contains(&entry_name) {
        return false;
    }
    !entry_name.starts_with('.') || entry_name == ".system"
}

pub const DEFAULT_ENABLED_PLUGIN_IDS: [&str; 13] = [
    "browser-use@zcode-plugins-official",
    "image-search@zcode-plugins-official",
    "documents@zcode-plugins-official",
    "pdf@zcode-plugins-official",
    "presentations@zcode-plugins-official",
    "spreadsheets@zcode-plugins-official",
    "node-repl-host@zcode-plugins-official",
    "skill-creator@zcode-plugins-official",
    "plugin-creator@zcode-plugins-official",
    "escode-guide@zcode-plugins-official",
    "visualize@zcode-plugins-official",
    "serial@zcode-plugins-official",
    "usb@zcode-plugins-official",
];

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
