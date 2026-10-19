//! `plugin-sync` channel 端到端（真 WebSocket + 真 RPC 帧；docs/specs/rust-u2-plugin-sync.md）。
mod support;

use base64::Engine as _;
use serde_json::{Value, json};
use support::*;

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    call(
        socket,
        id,
        "plugin-sync",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(socket, id).await
}

fn archive_of(value: &Wire) -> Vec<u8> {
    let Wire::Object(object) = value else {
        panic!("answer must be an object");
    };
    let text = object["archive"]["base64"]
        .as_str()
        .expect("archive base64");
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn plugin(root: &std::path::Path, name: &str) {
    write(
        &root.join(".escode-plugin/plugin.json"),
        &json!({"name": name, "version": "1.0.0"}).to_string(),
    );
    write(&root.join("README.md"), "# readme\n");
}

fn source_home(root: &std::path::Path) -> std::path::PathBuf {
    let home = root.join("home");
    plugin(&home.join("src/alpha"), "alpha");
    plugin(&home.join("src/beta"), "beta");
    write(
        &home.join(".escode/cli/config.json"),
        &json!({"features": {"mcp": true}, "plugins": {"dirs": [
            home.join("src/alpha").to_string_lossy(),
            home.join("src/beta").to_string_lossy()
        ]}})
        .to_string(),
    );
    home
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_sync_members_round_trip_over_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let home = source_home(dir.path());
    let import_home = dir.path().join("import-home");
    std::fs::create_dir_all(&import_home).unwrap();
    let surface = start(&home).await;
    let mut socket = open(&surface).await;

    let listed = call_json(&mut socket, 1, "listLocalUserPluginCandidates", json!({}))
        .await
        .into_object()
        .expect("candidates");
    let candidates = listed["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["name"], json!("alpha"));
    let alpha_id = candidates[0]["id"].as_str().unwrap().to_owned();

    let statuses = call_json(
        &mut socket,
        2,
        "listRemoteUserPluginStatuses",
        json!({"plugins": [
            {"pluginId": "gamma@inline", "directoryName": "remote-alpha"},
            {"pluginId": "beta@inline", "directoryName": "remote-beta"}
        ]}),
    )
    .await
    .into_object()
    .expect("statuses");
    assert_eq!(statuses["statuses"][0]["exists"], json!(false));
    assert_eq!(statuses["statuses"][1]["reason"], json!("samePluginId"));

    let exported = call_json(
        &mut socket,
        3,
        "exportPluginsArchive",
        json!({"pluginIds": [alpha_id]}),
    )
    .await;
    let archive = archive_of(&exported);
    assert_eq!(&archive[..2], &[0x1f, 0x8b]);

    let access = call_json(
        &mut socket,
        4,
        "checkRemoteUserPluginWriteAccess",
        json!({}),
    )
    .await
    .into_object()
    .expect("access");
    assert_eq!(access["ok"], json!(true));

    // 远端（另一个 home）导入；同 home 会因 samePluginId 跳过。
    let import_surface = start(&import_home).await;
    let mut remote = open(&import_surface).await;
    let payload = json!({
        "__escode_rpc_nested_uint8array_v1": true,
        "base64": base64::engine::general_purpose::STANDARD.encode(&archive),
    });
    let imported = call_json(
        &mut remote,
        1,
        "importPluginsArchive",
        json!({"archive": payload, "overwrite": false}),
    )
    .await
    .into_object()
    .expect("import");
    assert_eq!(
        imported["results"][0]["status"],
        json!("synced"),
        "{imported}"
    );
    assert!(
        import_home
            .join(".escode/plugins/alpha/.escode-plugin/plugin.json")
            .exists()
    );
    let again = call_json(
        &mut remote,
        2,
        "importPluginsArchive",
        json!({
            "archive": json!({
                "__escode_rpc_nested_uint8array_v1": true,
                "base64": base64::engine::general_purpose::STANDARD.encode(&archive),
            }),
            "overwrite": false,
        }),
    )
    .await
    .into_object()
    .expect("import again");
    assert_eq!(again["results"][0]["status"], json!("skipped"));

    let bad = call_json(
        &mut remote,
        3,
        "importPluginsArchive",
        json!({"archive": json!({"__escode_rpc_nested_uint8array_v1": true, "base64": "AQID"})}),
    )
    .await
    .into_object()
    .expect("error");
    assert_eq!(bad["name"], json!("Error"), "{bad}");
}

#[tokio::test(flavor = "multi_thread")]
async fn marketplace_source_archive_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let home = source_home(dir.path());
    let source_root = dir.path().join("market");
    plugin(&source_root.join("plugins/alpha"), "alpha");
    write(
        &source_root.join("marketplace.json"),
        &json!({
            "name": "test-market",
            "metadata": {"pluginRoot": "plugins"},
            "plugins": [{"name": "alpha", "source": "./alpha", "dependencies": []}]
        })
        .to_string(),
    );
    let surface = start(&home).await;
    let mut socket = open(&surface).await;
    let exported = call_json(
        &mut socket,
        1,
        "exportMarketplaceSourceArchive",
        json!({
            "marketplaceId": "test-market",
            "pluginNames": ["alpha"],
            "source": {"source": "directory", "path": source_root.to_string_lossy()}
        }),
    )
    .await
    .into_object()
    .expect("marketplace export");
    assert_eq!(exported["pluginNames"], json!(["alpha"]));
    let archive = archive_of(&Wire::Object(exported));

    let import_home = dir.path().join("import-home");
    std::fs::create_dir_all(&import_home).unwrap();
    let import_surface = start(&import_home).await;
    let mut remote = open(&import_surface).await;
    let imported = call_json(
        &mut remote,
        1,
        "importMarketplaceSourceArchive",
        json!({
            "archive": json!({
                "__escode_rpc_nested_uint8array_v1": true,
                "base64": base64::engine::general_purpose::STANDARD.encode(&archive),
            })
        }),
    )
    .await
    .into_object()
    .expect("marketplace import");
    assert_eq!(imported["status"], json!("synced"), "{imported}");
    assert!(
        std::path::Path::new(imported["path"].as_str().unwrap())
            .join("marketplace.json")
            .exists()
    );
}
