//! 插件来源解析（`rust-u2-commands.md` §1）：候选根、清单格式与启用判定。
use super::*;

#[test]
fn config_defaults_match_the_ts_shape() {
    let config = config_from(&serde_json::json!({}));
    assert!(config.enabled, "缺省启用");
    assert_eq!(config.storage_dir, "~/.escode");
    assert!(config.dirs.is_empty());
    assert!(config.suppressed_builtins.is_empty());
    assert!(config.enabled_plugins.is_empty());
    // 非字符串条目被忽略；storage.dir 全空白回退缺省。
    let config = config_from(&serde_json::json!({
        "storage": {"dir": "   "},
        "plugins": {"dirs": ["/a", 3], "enabled": false, "enabledPlugins": {"p@m": true, "q@m": "no"}}
    }));
    assert!(!config.enabled);
    assert_eq!(config.dirs, vec!["/a".to_owned()]);
    assert_eq!(config.storage_dir, "~/.escode");
    assert_eq!(config.enabled_plugins.len(), 1);
}

#[test]
fn resolve_inside_rejects_absolute_and_escaping_paths() {
    let root = Path::new("/plugins/demo");
    assert_eq!(
        resolve_inside(root, "commands"),
        Some(PathBuf::from("/plugins/demo/commands"))
    );
    // 留在根内（含根本身）。
    assert_eq!(
        resolve_inside(root, "."),
        Some(PathBuf::from("/plugins/demo"))
    );
    assert_eq!(
        resolve_inside(root, "a/../b"),
        Some(PathBuf::from("/plugins/demo/b"))
    );
    // 绝对路径一律拒绝；`..` 逃出根也拒绝。
    assert_eq!(resolve_inside(root, "/etc"), None);
    assert_eq!(resolve_inside(root, "../other"), None);
    assert_eq!(resolve_inside(root, "a/../../other"), None);
}

#[test]
fn plugin_names_follow_the_ts_pattern() {
    for valid in ["demo", "a1", "my-plugin", "a.b_c-d", &"a".repeat(128)] {
        assert!(is_plugin_name(valid), "应接受: {valid}");
    }
    for invalid in [
        "",
        "Demo",
        "-lead",
        "_lead",
        ".lead",
        "a b",
        &"a".repeat(129),
        "中文",
    ] {
        assert!(!is_plugin_name(invalid), "应拒绝: {invalid}");
    }
}
