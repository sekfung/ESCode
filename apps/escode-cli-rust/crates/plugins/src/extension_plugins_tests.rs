//! 发现层回归：bundled-marketplace 是目录（catalog），`cachePath` 指向“理应安放”的位置。
//! Node `scanOfficialCache` 把分片里的每个 `cachePath` 都当候选、不查存在性；seed 未覆盖的可选
//! 随包插件（如 rea）因此由 loader 产出 `plugin_root_not_found`。Rust 必须保留这条诊断，否则
//! `plugins/list` 的发现诊断与 Node 不等（CI run `41270183483` 的 plugins-hooks / plugins-diagnostics）。
use super::extension_plugins::discover;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn marketplace_cache_path_is_discovered_even_when_missing_like_node() {
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
    let missing_diagnostic = diagnostics
        .iter()
        .find(|item| item["code"] == "plugin_root_not_found")
        .expect("missing cachePath must report plugin_root_not_found like Node");
    assert_eq!(missing_diagnostic["severity"], "warning");
    assert_eq!(
        missing_diagnostic["message"].as_str().unwrap(),
        format!("Plugin root does not exist: {}", missing.display())
    );
}

#[tokio::test]
async fn installed_plugin_file_and_root_recover_atomic_backups_like_node() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("cli").join("plugins");
    let target = storage
        .join("cache")
        .join("recover-market")
        .join("recover-plugin")
        .join("1.0.0");
    tokio::fs::create_dir_all(target.join(".escode-plugin"))
        .await
        .unwrap();
    tokio::fs::write(
        target.join(".escode-plugin").join("plugin.json"),
        r#"{"name":"recover-plugin","version":"1.0.0"}"#,
    )
    .await
    .unwrap();
    tokio::fs::rename(&target, target.parent().unwrap().join(".1.0.0.backup"))
        .await
        .unwrap();

    let installed = storage.join("installed_plugins.json");
    tokio::fs::write(
        &installed,
        format!(
            r#"{{"version":1,"plugins":[{{"id":"recover-plugin@recover-market","name":"recover-plugin","marketplace":"recover-market","version":"1.0.0","installPath":{},"installedAt":"2026-01-01T00:00:00.000Z","scope":"user"}}]}}"#,
            serde_json::to_string(&target).unwrap()
        ),
    )
    .await
    .unwrap();
    tokio::fs::rename(&installed, storage.join(".installed_plugins.json.backup"))
        .await
        .unwrap();

    let config = json!({
        "storage": { "dir": dir.path() },
        "plugins": {
            "enabled": true,
            "dirs": [],
            "enabledPlugins": { "recover-plugin@recover-market": true },
        },
    });
    let (plugins, _) = discover(dir.path(), &config, &CancellationToken::new())
        .await
        .unwrap();

    assert!(
        plugins
            .iter()
            .any(|plugin| plugin.id == "recover-plugin@recover-market"),
        "installed plugin should be discovered after recovery: {:?}",
        plugins.iter().map(|p| p.id.clone()).collect::<Vec<_>>()
    );
    assert!(installed.exists());
    assert!(target.exists());
}
