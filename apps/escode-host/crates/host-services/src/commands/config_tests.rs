//! 启用态覆盖表与用户配置（`rust-u2-commands.md` §2，TS `commandsService.ts` 的 config 一族）。
use super::*;

fn record(value: serde_json::Value) -> Value {
    value
}

#[test]
fn disable_writes_the_override_and_enable_removes_it() {
    let base = record(serde_json::json!({"locale": "zh-CN"}));
    let disabled = set_override(&base, "/c/a.md", false);
    assert_eq!(
        disabled,
        serde_json::json!({"locale": "zh-CN", "command": {"/c/a.md": {"enable": false}}}),
        "禁用写最小覆盖；其余配置键原样保留"
    );
    assert!(!is_enabled(&enabled_overrides(&disabled), "/c/a.md"));

    // 重新启用 = 删键；`command` 空了连键一起删。
    let reenabled = set_override(&disabled, "/c/a.md", true);
    assert_eq!(reenabled, base);
    assert!(reenabled.get("command").is_none(), "空 command 不留空对象");
}

#[test]
fn overrides_ignore_non_boolean_entries() {
    let config = serde_json::json!({
        "command": {
            "/a.md": {"enable": false},
            "/b.md": {"enable": "no"},
            "/c.md": {"other": true},
            "/d.md": true
        }
    });
    let overrides = enabled_overrides(&config);
    assert_eq!(overrides.len(), 1);
    assert!(!is_enabled(&overrides, "/a.md"));
    // 没有覆盖即启用（缺省 true）。
    assert!(is_enabled(&overrides, "/b.md"));
    // 非对象的 `command` 也安全。
    assert!(enabled_overrides(&serde_json::json!({"command": 3})).is_empty());
}

#[tokio::test]
async fn config_round_trips_with_the_ts_layout() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // 读不到 → 空对象（不抛）。
    assert_eq!(read_config(home).await, serde_json::json!({}));

    write_config(home, &serde_json::json!({"locale": "en-US"}))
        .await
        .unwrap();
    let path = user_cli_config_path(home);
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(
        raw, "{\n  \"locale\": \"en-US\"\n}\n",
        "2 空格缩进 + 结尾换行"
    );
    assert_eq!(
        read_config(home).await,
        serde_json::json!({"locale": "en-US"})
    );

    // 坏 JSON / 非对象 → 空对象。
    tokio::fs::write(&path, "{oops").await.unwrap();
    assert_eq!(read_config(home).await, serde_json::json!({}));
    tokio::fs::write(&path, "[]").await.unwrap();
    assert_eq!(read_config(home).await, serde_json::json!({}));
}
