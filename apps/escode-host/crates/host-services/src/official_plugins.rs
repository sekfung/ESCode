//! 官方插件的默认启用集合（TS `@escode/shared` 的 `DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS`）。
//!
//! **注意**：`packages/services/src/plugins/officialPluginCache.ts` 还有一份**更短**的同名集合
//! （5 条），那是 services 内部的旧副本；`commands` 与 `subagents` 都从 `@escode/shared` 取，
//! 即本表（13 条）。两份不一致是 TS 侧的既有分叉，这里只复刻被 RPC 面实际使用的那一份。
pub const DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS: [&str; 13] = [
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

/// 官方市场 id（与缓存目录同名）。
pub const OFFICIAL_PLUGIN_MARKETPLACE: &str = "zcode-plugins-official";

pub fn is_default_enabled(plugin_id: &str) -> bool {
    DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS.contains(&plugin_id)
}

#[cfg(test)]
#[path = "official_plugins_tests.rs"]
mod tests;
