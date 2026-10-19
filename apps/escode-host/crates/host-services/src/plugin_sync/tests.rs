//! plugin-sync 单测：候选/manifest、归档往返、导入跳过与配置写入。

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use super::archive::{ArchiveEntry, create_archive, extract_archive};
use super::candidates;
use super::service::PluginSyncService;
use crate::skills::Compare;

fn compare() -> Compare {
    Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn plugin(root: &Path, name: &str, with_skill: bool) {
    write(
        &root.join(".escode-plugin/plugin.json"),
        &json!({
            "name": name,
            "description": format!("{name} description"),
            "version": "1.0.0",
        })
        .to_string(),
    );
    write(&root.join("README.md"), "# readme\n");
    if with_skill {
        write(&root.join("skills/demo/SKILL.md"), "# skill\n");
    }
}

fn home_with_plugins() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let alpha = home.join("src/alpha");
    let beta = home.join("src/beta");
    plugin(&alpha, "alpha", false);
    plugin(&beta, "beta", true);
    write(
        &home.join(".escode/cli/config.json"),
        &json!({
            "features": {"mcp": true},
            "plugins": {
                "dirs": [alpha.to_string_lossy(), beta.to_string_lossy()],
                "enabledPlugins": {"beta@inline": false}
            }
        })
        .to_string(),
    );
    (dir, home)
}

#[tokio::test]
async fn candidates_follow_manifest_and_config_shape() {
    let (_dir, home) = home_with_plugins();
    let service = PluginSyncService::new(home, compare());
    let result = service.list_local_user_plugin_candidates().await.unwrap();
    let candidates = result["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["name"], json!("alpha"));
    assert_eq!(candidates[0]["pluginId"], json!("alpha@inline"));
    assert_eq!(candidates[0]["enabled"], json!(true));
    assert_eq!(candidates[1]["enabledOverride"], json!(false));
    assert_eq!(candidates[1]["componentTypes"], json!(["skills"]));
    assert_eq!(result["maxArchiveBytes"], json!(52_428_800));
}

#[tokio::test]
async fn export_and_import_round_trip_adds_config_dir() {
    let (_dir, home) = home_with_plugins();
    let service = PluginSyncService::new(home.clone(), compare());
    let listed = service.list_local_user_plugin_candidates().await.unwrap();
    let alpha = listed["candidates"][0]["id"].as_str().unwrap().to_owned();
    let exported = service
        .export_plugins_archive(&json!({"pluginIds": [alpha]}))
        .await
        .unwrap();
    assert_eq!(exported.archive_bytes, exported.archive.len() as u64);
    assert_eq!(&exported.archive[..2], &[0x1f, 0x8b]);

    let remote_home = _dir.path().join("remote-home");
    write(
        &remote_home.join(".escode/cli/config.json"),
        &json!({"features": {"mcp": true}}).to_string(),
    );
    let remote = PluginSyncService::new(remote_home.clone(), compare());
    let imported = remote
        .import_plugins_archive(exported.archive.clone(), false)
        .await
        .unwrap();
    assert_eq!(imported["results"][0]["status"], json!("synced"));
    let target = remote_home.join(".escode/plugins/alpha");
    assert!(target.join(".escode-plugin/plugin.json").exists());
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(remote_home.join(".escode/cli/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["features"]["mcp"], json!(true));
    assert!(
        config["plugins"]["dirs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(target.to_string_lossy().as_ref()))
    );

    let again = remote
        .import_plugins_archive(exported.archive, false)
        .await
        .unwrap();
    assert_eq!(again["results"][0]["status"], json!("skipped"));
}

#[tokio::test]
async fn import_rejects_overwrite_and_bad_archives() {
    let (_dir, home) = home_with_plugins();
    let service = PluginSyncService::new(home, compare());
    let error = service
        .import_plugins_archive(vec![1, 2, 3], true)
        .await
        .unwrap_err();
    assert_eq!(error, "plugin sync overwrite is not supported");
    let error = service
        .import_plugins_archive(vec![1, 2, 3], false)
        .await
        .unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn archive_round_trips_and_restores_modes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("plugin");
    write(&source.join("SKILL.md"), "# skill\n");
    write(&source.join("bin/run.sh"), "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            source.join("bin/run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let archive = create_archive(
        &[ArchiveEntry::Source {
            source_path: source,
            archive_path: "group/plugin".to_owned(),
        }],
        "{\n  \"plugins\": []\n}\n",
        &compare(),
    )
    .unwrap();
    assert_eq!(&archive[..2], &[0x1f, 0x8b]);
    let target = dir.path().join("out");
    extract_archive(&archive, &target, Some(1024 * 1024)).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("group/plugin/SKILL.md")).unwrap(),
        "# skill\n"
    );
    assert!(target.join("group/plugin/bin/run.sh").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target.join("group/plugin/bin/run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert!(mode & 0o100 != 0, "可执行位按 header 恢复: {mode:o}");
    }
}

#[test]
fn archive_rejects_traversal_and_size_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let archive = create_archive(
        &[ArchiveEntry::Memory {
            archive_path: "ok.txt".to_owned(),
            content: b"0123456789".to_vec(),
            mode: 0o644,
            mtime_ms: Some(0),
        }],
        "{}",
        &compare(),
    )
    .unwrap();
    let error = extract_archive(&archive, dir.path(), Some(5)).unwrap_err();
    assert!(error.contains("exceeds limit"), "{error}");

    // 路径穿越由 tar 头直接构造：手工改一个条目名。
    let mut tar = Vec::new();
    tar.extend_from_slice(&super::archive::test_header(
        "../escape.txt",
        0o644,
        0,
        1,
        b'0',
    ));
    tar.extend_from_slice(b"x");
    tar.extend_from_slice(&[0_u8; 511]);
    tar.extend_from_slice(&[0_u8; 1024]);
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    encoder.write_all(&tar).unwrap();
    let archive = encoder.finish().unwrap();
    let error = extract_archive(&archive, dir.path(), None).unwrap_err();
    assert!(error.contains("unsafe plugin archive path"), "{error}");
    assert!(!dir.path().join("escape.txt").exists());
}

#[test]
fn configured_paths_and_directory_names_are_stable() {
    let (_dir, home) = home_with_plugins();
    let paths = candidates::configured_paths(&home).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.contains_key("alpha@inline"));
    let config = candidates::read_user_config(&home).unwrap();
    assert_eq!(
        config["plugins"]["enabledPlugins"]["beta@inline"],
        json!(false)
    );
}
