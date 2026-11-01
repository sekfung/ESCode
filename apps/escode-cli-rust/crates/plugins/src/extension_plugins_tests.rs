//! 发现层回归：bundled-marketplace 的 `cachePath` 是目录声明的“理应安放”位置，seed 失败时并不存在。
//! Node 只发现实际存在的根；Rust 若把缺失路径也当候选，会多出无 pluginId 的 `plugin_root_not_found`
//! （CI run `41264933512` 的 `escode-cli-rust-plugins-hooks.test.ts` 差异）。
use super::extension_plugins::discover;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn marketplace_cache_path_must_exist_to_be_discovered() {
    let dir = tempfile::tempdir().unwrap();
    // `storage()` 把不以 cli 结尾的根补成 <root>/cli/plugins。
    let storage = dir.path().join("cli").join("plugins");
    let market = storage.join("marketplaces").join("zcode-plugins-official");
    tokio::fs::create_dir_all(&market).await.unwrap();

    let present = storage
        .join("cache")
        .join("zcode-plugins-official")
        .join("present")
        .join("0.1.0");
    tokio::fs::create_dir_all(present.join(".escode-plugin"))
        .await
        .unwrap();
    tokio::fs::write(
        present.join(".escode-plugin").join("plugin.json"),
        r#"{"name":"present","version":"0.1.0"}"#,
    )
    .await
    .unwrap();

    let missing = storage
        .join("cache")
        .join("zcode-plugins-official")
        .join("missing")
        .join("0.1.0");
    let partition = json!({
        "version": 1,
        "manifest": {
            "name": "zcode-plugins-official",
            "version": 1,
            "plugins": [
                {"name": "present", "version": "0.1.0", "cachePath": present},
                {"name": "missing", "version": "0.1.0", "cachePath": missing},
            ],
        },
    });
    tokio::fs::write(
        market.join("bundled-marketplace.json"),
        serde_json::to_vec(&partition).unwrap(),
    )
    .await
    .unwrap();

    let config = json!({
        "storage": { "dir": dir.path() },
        "plugins": { "enabled": true, "dirs": [] },
    });
    let (plugins, diagnostics) = discover(dir.path(), &config, &CancellationToken::new())
        .await
        .unwrap();

    assert!(
        plugins
            .iter()
            .any(|plugin| plugin.id == "present@zcode-plugins-official"),
        "present cache root should be discovered: {:?}",
        plugins.iter().map(|p| p.id.clone()).collect::<Vec<_>>()
    );
    assert!(
        !plugins
            .iter()
            .any(|plugin| plugin.id == "missing@zcode-plugins-official")
    );
    assert!(
        !diagnostics
            .iter()
            .any(|item| item["code"] == "plugin_root_not_found"),
        "{diagnostics:?}"
    );
}
