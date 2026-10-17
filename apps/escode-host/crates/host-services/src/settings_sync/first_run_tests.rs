//! 首启提示状态（`rust-u2-sync.md` §3）。
use super::*;

#[tokio::test]
async fn state_defaults_to_false_and_mark_preserves_other_settings() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        state(home.path()).await.unwrap(),
        serde_json::json!({"handled": false})
    );

    let settings = home.path().join(".escode/v2/setting.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"other":1,"settingsSyncFirstRunPromptHandled":false}"#,
    )
    .unwrap();

    mark_handled(home.path()).await.unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(parsed["other"], serde_json::json!(1));
    assert_eq!(
        parsed["settingsSyncFirstRunPromptHandled"],
        serde_json::json!(true)
    );
    assert_eq!(
        state(home.path()).await.unwrap(),
        serde_json::json!({"handled": true})
    );

    // 只认严格 true（TS `=== true`）。
    std::fs::write(&settings, r#"{"settingsSyncFirstRunPromptHandled":"true"}"#).unwrap();
    assert_eq!(
        state(home.path()).await.unwrap(),
        serde_json::json!({"handled": false})
    );
}
