//! mcp-sync 服务（`rust-u2-sync.md` §2）。
use super::*;

fn compare() -> Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn service(home: &std::path::Path) -> McpSyncService {
    McpSyncService::new(home.to_path_buf(), compare())
}

fn seed_source(home: &std::path::Path) {
    write(
        &home.join(".escode/cli/config.json"),
        &serde_json::json!({
            "mcp": {"servers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem",
                             home.join("project").to_string_lossy()],
                },
                "remote-only": {"url": "https://example.test/mcp", "enabled": false},
            }},
        })
        .to_string(),
    );
}

#[tokio::test]
async fn candidates_statuses_and_export_use_the_effective_user_records() {
    let home = tempfile::tempdir().unwrap();
    seed_source(home.path());
    let service = service(home.path());

    let listed = service.list_local_user_mcp_candidates().await.unwrap();
    let candidates = listed["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["name"], serde_json::json!("filesystem"));
    assert_eq!(candidates[0]["enabled"], serde_json::json!(true));
    assert_eq!(candidates[1]["enabled"], serde_json::json!(false));
    assert_eq!(
        listed["localHomeDir"],
        serde_json::json!(home.path().to_string_lossy())
    );

    let ids: Vec<_> = candidates
        .iter()
        .map(|candidate| candidate["id"].as_str().unwrap().to_owned())
        .collect();
    let exported = service
        .export_mcp_servers(&serde_json::json!({"serverIds": ids}))
        .await
        .unwrap();
    assert_eq!(exported["servers"].as_array().unwrap().len(), 2);
    assert_eq!(
        exported["servers"][1]["config"],
        serde_json::json!({"url": "https://example.test/mcp", "enabled": false})
    );

    let statuses = service
        .list_remote_user_mcp_statuses(&serde_json::json!({"names": ["filesystem", "nope"]}))
        .await
        .unwrap();
    assert_eq!(statuses["statuses"][0]["exists"], serde_json::json!(true));
    assert_eq!(statuses["statuses"][1]["exists"], serde_json::json!(false));

    let error = service
        .export_mcp_servers(&serde_json::json!({"serverIds": ["missing"]}))
        .await
        .unwrap_err();
    assert_eq!(error, "mcp sync candidate not found: missing");

    assert_eq!(
        service.check_remote_user_mcp_write_access().await.unwrap()["ok"],
        serde_json::json!(true)
    );
}

#[tokio::test]
async fn import_rewrites_filesystem_paths_and_skips_duplicates() {
    let source_home = tempfile::tempdir().unwrap();
    seed_source(source_home.path());
    let source = service(source_home.path());
    let ids: Vec<String> = source.list_local_user_mcp_candidates().await.unwrap()["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["id"].as_str().unwrap().to_owned())
        .collect();
    let exported = source
        .export_mcp_servers(&serde_json::json!({"serverIds": ids}))
        .await
        .unwrap();

    let target_home = tempfile::tempdir().unwrap();
    let target = service(target_home.path());
    let imported = target
        .import_mcp_servers(
            &serde_json::json!({
                "servers": exported["servers"],
                "localHomeDir": source_home.path().to_string_lossy(),
                "localWorkspacePath": source_home.path().join("project").to_string_lossy(),
                "remoteWorkspacePath": target_home.path().join("project").to_string_lossy(),
            }),
            false,
        )
        .await
        .unwrap();
    assert_eq!(imported["results"].as_array().unwrap().len(), 2);
    assert_eq!(
        imported["results"][0]["status"],
        serde_json::json!("synced")
    );
    assert_eq!(
        imported["results"][1]["status"],
        serde_json::json!("synced")
    );

    let config = read(&target_home.path().join(".escode/cli/config.json"));
    assert_eq!(
        config["mcp"]["servers"]["filesystem"]["args"][2],
        serde_json::json!(target_home.path().join("project").to_string_lossy())
    );
    assert!(
        config["mcp"]["servers"]["filesystem"]
            .get("enabled")
            .is_none(),
        "启用态不落冗余字段"
    );
    assert_eq!(
        config["mcp"]["servers"]["remote-only"]["enabled"],
        serde_json::json!(false)
    );

    let again = target
        .import_mcp_servers(&serde_json::json!({"servers": exported["servers"]}), false)
        .await
        .unwrap();
    for item in again["results"].as_array().unwrap() {
        assert_eq!(item["status"], serde_json::json!("skipped"));
    }

    assert_eq!(
        target
            .import_mcp_servers(&serde_json::json!({"servers": []}), true)
            .await
            .unwrap_err(),
        "mcp sync overwrite is not supported"
    );
}

#[tokio::test]
async fn import_skips_a_name_that_exists_in_the_agents_root() {
    let source_home = tempfile::tempdir().unwrap();
    seed_source(source_home.path());
    let source = service(source_home.path());
    let listed = source.list_local_user_mcp_candidates().await.unwrap();
    let id = listed["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["name"] == serde_json::json!("filesystem"))
        .map(|candidate| candidate["id"].as_str().unwrap().to_owned())
        .unwrap();
    let exported = source
        .export_mcp_servers(&serde_json::json!({"serverIds": [id]}))
        .await
        .unwrap();

    let target_home = tempfile::tempdir().unwrap();
    let agents = target_home.path().join(".agents/mcp.json");
    write(
        &agents,
        &serde_json::json!({
            "mcpServers": {"filesystem": {"command": "agent-version"}},
        })
        .to_string(),
    );
    let target = service(target_home.path());
    let imported = target
        .import_mcp_servers(&serde_json::json!({"servers": exported["servers"]}), false)
        .await
        .unwrap();
    assert_eq!(
        imported["results"][0]["status"],
        serde_json::json!("skipped")
    );
    assert_eq!(
        imported["results"][0]["path"],
        serde_json::json!(agents.to_string_lossy())
    );
    assert!(!target_home.path().join(".escode/cli/config.json").exists());

    // `.agents` 有内容时候选也来自它。
    let listed = target.list_local_user_mcp_candidates().await.unwrap();
    assert_eq!(
        listed["candidates"][0]["source"],
        serde_json::json!("agents")
    );
}
