//! 官方插件默认启用表（`rust-u2-subagents.md` §6.5-4）。
use super::*;

#[test]
fn default_enabled_set_matches_the_shared_table() {
    assert_eq!(DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS.len(), 13);
    assert!(is_default_enabled("browser-use@zcode-plugins-official"));
    assert!(is_default_enabled("usb@zcode-plugins-official"));
    // `computer-use` 明确不在表内（默认关闭）。
    assert!(!is_default_enabled("computer-use@zcode-plugins-official"));
    // 非官方市场一律不默认启用。
    assert!(!is_default_enabled("browser-use@inline"));
}
