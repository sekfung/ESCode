//! hooks 域单测：发现顺序、legacy 解析、save 字节格式、trust fail-closed。

use std::path::Path;

use serde_json::{Value, json};

use super::HooksService;
use super::config::{self, HookEvent, RuntimeRoot};

async fn write(path: &Path, text: &str) {
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, text).await.unwrap();
}

async fn write_json(path: &Path, value: &Value) {
    write(path, &serde_json::to_string(value).unwrap()).await;
}

#[tokio::test]
async fn loads_project_user_and_legacy_sources_in_typescript_order() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    tokio::fs::create_dir_all(workspace.join(".git"))
        .await
        .unwrap();
    write_json(
        &home.join(".escode/cli/config.json"),
        &json!({"hooks": {"enabled": true, "events": {"SessionStart": [{"matcher": "startup", "hooks": [
            {"type": "process", "command": "node", "args": ["user.mjs"], "customFlag": true}
        ]}]}}}),
    )
    .await;
    write_json(
        &workspace.join(".escode/config.json"),
        &json!({"hooks": {"enabled": true, "events": {"PreToolUse": [{"matcher": "Bash", "hooks": [
            {"type": "process", "command": "node", "args": ["project.mjs"], "timeoutMs": 5000}
        ]}]}}}),
    )
    .await;
    write_json(
        &workspace.join(".claude/settings.json"),
        &json!({"hooks": {"PostToolUse": [{"matcher": "Write", "hooks": [
            {"type": "command", "command": "echo claude"}
        ]}]}}),
    )
    .await;
    write_json(
        &home.join(".agents/settings.json"),
        &json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "echo agents"}
        ]}]}}),
    )
    .await;

    let result = HooksService::new(home.clone())
        .load(&json!({"workspacePath": workspace}))
        .await
        .unwrap();
    let hooks = result["hooks"].as_array().unwrap();
    let commands: Vec<&str> = hooks
        .iter()
        .map(|hook| hook["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        commands,
        ["node", "echo claude", "node", "echo agents"],
        "项目 → 项目 legacy → 用户 → 用户 legacy"
    );
    assert_eq!(hooks[0]["timeout"], json!(5));
    assert_eq!(hooks[2]["args"], json!(["user.mjs"]));
    assert_eq!(hooks[2]["custom"], json!({"customFlag": true}));
    assert_eq!(hooks[1]["enabled"], json!(false));
    assert_eq!(hooks[1]["editable"], json!(false));
    assert_eq!(result["hooksEnabled"], json!(true));
    let snapshot = &result["workspaceHookSnapshot"];
    assert_eq!(snapshot["schemaVersion"], json!(1));
    assert_eq!(snapshot["sourceFiles"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["hooks"].as_array().unwrap().len(), 1);
    // TS oracle（真实 createHooksService 在同一夹具上的输出，2026-10-19）：
    // 直接比对规范摘要，防止数组顺序/编码漂移让既有 trust 记录失效。
    assert_eq!(
        snapshot["hooks"][0]["hookDeclarationDigest"],
        json!("0724c613fe3d6c94d698d8b0eff02c0c6c08833122c9164f37234762ab63fc6a")
    );
    assert_eq!(
        snapshot["bundleDigest"],
        json!("e95f1c11577a2ab816df613e80fe7dcb5142412c6af3785f8efc03f2e219814c")
    );
}

#[tokio::test]
async fn save_preserves_unknown_keys_and_writes_typescript_shaped_json() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    write_json(
        &home.join(".escode/cli/config.json"),
        &json!({"features": {"mcp": true}}),
    )
    .await;
    let service = HooksService::new(home.clone());
    service
        .save(&json!({
            "workspacePath": workspace,
            "hooks": [
                {
                    "id": "keep-user", "event": "Stop", "type": "process",
                    "command": "node", "args": ["stop.mjs"], "enabled": true,
                    "custom": {"extra": "user"},
                    "location": {"source": "escode", "scope": "user"}
                },
                {
                    "id": "keep-project", "event": "PreToolUse", "type": "process",
                    "command": "node", "enabled": true,
                    "location": {"source": "escode", "scope": "project"}
                },
                {
                    "id": "skip-agents", "event": "Stop", "type": "command",
                    "command": "echo agents", "enabled": true,
                    "location": {"source": "agents", "scope": "user"}
                }
            ]
        }))
        .await
        .unwrap();

    let user_text = tokio::fs::read_to_string(home.join(".escode/cli/config.json"))
        .await
        .unwrap();
    assert_eq!(
        user_text,
        r#"{
  "features": {
    "mcp": true
  },
  "hooks": {
    "enabled": true,
    "events": {
      "Stop": [
        {
          "hooks": [
            {
              "extra": "user",
              "type": "process",
              "command": "node",
              "enabled": true,
              "args": [
                "stop.mjs"
              ]
            }
          ]
        }
      ]
    }
  }
}
"#
    );
    let project_text = tokio::fs::read_to_string(workspace.join(".escode/config.json"))
        .await
        .unwrap();
    assert_eq!(
        project_text,
        r#"{
  "hooks": {
    "enabled": true,
    "events": {
      "PreToolUse": [
        {
          "hooks": [
            {
              "type": "process",
              "command": "node",
              "enabled": true
            }
          ]
        }
      ]
    }
  }
}
"#
    );
}

#[tokio::test]
async fn command_hooks_round_trip_async_shell_and_timeout() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    let service = HooksService::new(home);
    service
        .save(&json!({
            "workspacePath": workspace,
            "hooks": [{
                "id": "command-hook", "event": "SessionStart", "matcher": "startup",
                "type": "command", "command": "./hook.sh", "async": true, "shell": "bash",
                "statusMessage": "Preparing", "timeout": 12, "enabled": true,
                "location": {"source": "escode", "scope": "project"}
            }]
        }))
        .await
        .unwrap();
    let loaded = service
        .load(&json!({"workspacePath": workspace}))
        .await
        .unwrap();
    assert_eq!(
        loaded["hooks"][0],
        json!({
            "id": "workspace-hook-0-SessionStart-0-0",
            "event": "SessionStart", "matcher": "startup", "type": "command",
            "command": "./hook.sh", "async": true, "shell": "bash",
            "statusMessage": "Preparing", "timeout": 12, "enabled": true,
            "editable": true,
            "configuredState": {
                "sourceRootEnabled": true, "declarationEnabled": true,
                "runtimeHooksEnabled": true, "configuredEnabled": true,
                "sourcePath": loaded["hooks"][0]["configuredState"]["sourcePath"]
            },
            "workspaceHook": {
                "sourceRootEnabled": true, "declarationEnabled": true,
                "runtimeHooksEnabled": true, "configuredEnabled": true,
                "sourcePath": loaded["hooks"][0]["workspaceHook"]["sourcePath"],
                "reviewItemId": "workspace-hook-0-SessionStart-0-0",
                "sourceFileIndex": 0,
                "workspaceIdentity": loaded["hooks"][0]["workspaceHook"]["workspaceIdentity"],
                "bundleDigest": loaded["hooks"][0]["workspaceHook"]["bundleDigest"],
                "hookDeclarationDigest": loaded["hooks"][0]["workspaceHook"]["hookDeclarationDigest"],
                "trustState": "pending_trust"
            },
            "location": {
                "source": "escode", "scope": "project",
                "directoryPath": loaded["hooks"][0]["location"]["directoryPath"],
                "projectPath": loaded["hooks"][0]["location"]["projectPath"]
            }
        })
    );
}

#[tokio::test]
async fn trust_store_corrupt_and_missing_are_fail_closed_but_distinguishable() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let workspace = root.path().join("workspace");
    tokio::fs::create_dir_all(workspace.join(".git"))
        .await
        .unwrap();
    write_json(
        &workspace.join(".escode/config.json"),
        &json!({"hooks": {"enabled": true, "events": {"Stop": [{"hooks": [
            {"type": "command", "command": "echo x"}
        ]}]}}}),
    )
    .await;
    let service = HooksService::new(home.clone());
    let missing = service
        .load(&json!({"workspacePath": workspace}))
        .await
        .unwrap();
    assert!(missing.get("trustStoreCorrupt").is_none());
    assert_eq!(
        missing["hooks"][0]["workspaceHook"]["trustState"],
        json!("pending_trust")
    );

    write(
        &home.join(".escode/security/workspace-hook-trust-v1.json"),
        "{broken",
    )
    .await;
    let corrupt = service
        .load(&json!({"workspacePath": workspace}))
        .await
        .unwrap();
    assert_eq!(corrupt["trustStoreCorrupt"], json!(true));
    assert_eq!(
        corrupt["hooks"][0]["workspaceHook"]["trustState"],
        json!("pending_trust")
    );
}

#[test]
fn schema_strictness_matches_zod() {
    assert!(config::parse_hooks_config(&json!({"unknown": true})).is_err());
    assert!(config::parse_hooks_config(&json!({"events": {"Nope": []}})).is_err());
    assert!(
        config::parse_hooks_config(&json!({"events": {"Stop": [{"matcher": "", "hooks": []}]}}))
            .is_err()
    );
    assert!(
        config::parse_hooks_config(&json!({"events": {"Stop": [{"hooks": [
            {"type": "command", "command": "x", "shell": ""}
        ]}]}}))
        .is_err()
    );
    assert!(config::parse_hooks_config(&json!({"timeoutMs": 0})).is_err());
    let parsed = config::parse_hooks_config(&json!({
        "enabled": true,
        "events": {"Stop": [{"hooks": [{"type": "process", "command": "x", "unknown": 1}]}]}
    }))
    .unwrap();
    let hook = &parsed.events[config::event_index(HookEvent::Stop)]
        .as_ref()
        .unwrap()[0]
        .hooks[0];
    assert_eq!(hook.custom["unknown"], json!(1));
}

#[test]
fn runtime_root_uses_last_defined_values_and_any_enabled() {
    let user = config::parse_hooks_config(&json!({"enabled": true, "timeoutMs": 1000})).unwrap();
    let project =
        config::parse_hooks_config(&json!({"timeoutMs": 2000, "maxOutputBytes": 4096})).unwrap();
    let root = RuntimeRoot::resolve([&user, &project]);
    assert!(root.enabled);
    assert_eq!(root.timeout_ms, 2000.0);
    assert_eq!(root.max_output_bytes, 4096.0);
}
