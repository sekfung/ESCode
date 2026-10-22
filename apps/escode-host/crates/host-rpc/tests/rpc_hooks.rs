//! `hooks` channel 端到端（真 WebSocket + 真 RPC 帧；docs/specs/rust-u2-hooks.md §9）。
//!
//! `loadHooks`/`saveHooks` 走 Host 本地文件域；`grantWorkspaceHookTrust` 走运行时转发。
//! 转发用例用最小 Node workflow host 桩，只验证 Host→runtime 的方法/参数与结果透传；
//! Agent authority 自身的策略/重新发现语义由 TS `workspace-hook-trust-protocol.test.ts` 覆盖。

mod support;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use support::*;

async fn start_hooks(
    home: &std::path::Path,
    data_base_dir: &std::path::Path,
    env: BTreeMap<String, String>,
) -> RpcSurface {
    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory = std::sync::Arc::new({
        let home = home.to_path_buf();
        move |workspace: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: home.clone(),
                env: env.clone(),
                argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: workspace.data_dir.clone().map(std::path::PathBuf::from),
                config: workspace.config.clone().map(std::path::PathBuf::from),
                legacy_import: escode_cli_rust::embed::LegacyImport::Disabled,
            }
        }
    });
    let channels = escode_host_rpc::domains::channels(
        home.to_path_buf(),
        Some(data_base_dir.to_path_buf()),
        instances.clone(),
        options,
    );
    escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, Some(instances))
        .await
        .expect("rpc surface starts")
}

async fn call_json(socket: &mut Socket, id: i64, method: &str, params: Value) -> Wire {
    call(
        socket,
        id,
        "hooks",
        method,
        &Wire::Array(vec![Wire::Object(params)]),
    )
    .await;
    read_response(socket, id).await
}

async fn write_json(path: &std::path::Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(value).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn load_save_and_grant_round_trip_over_the_rpc_surface() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data");
    let workspace = dir.path().join("workspace");
    for path in [&home, &data_base_dir, &workspace] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    write_json(
        &workspace.join(".escode/config.json"),
        &json!({"hooks": {"enabled": true, "events": {"PreToolUse": [{"matcher": "Bash", "hooks": [
            {"type": "process", "command": "node", "args": ["hook.mjs"]}
        ]}]}}}),
    )
    .await;
    write_json(
        &home.join(".escode/cli/config.json"),
        &json!({"features": {"mcp": true}}),
    )
    .await;

    // 最小 workflow host 桩：记录请求并回成功/拒绝。
    let stub = dir.path().join("workflow-host.cjs");
    let log = dir.path().join("workflow-host.log");
    // 子进程继承的是 Host 进程 env（不是实例 env），因此日志路径直接写进桩脚本。
    let log_literal = serde_json::to_string(&log.to_string_lossy()).unwrap();
    std::fs::write(
        &stub,
        format!(
            r#"
const fs = require("node:fs");
const readline = require("node:readline");
const log = {log_literal};
const rl = readline.createInterface({{ input: process.stdin }});
rl.on("close", () => process.exit(0));
rl.on("line", (line) => {{
  const message = JSON.parse(line);
  fs.appendFileSync(log, JSON.stringify(message) + "\n");
  const params = message.params && message.params.params ? message.params.params : {{}};
  const result = params.hookDeclarationDigest === "reject"
    ? {{ accepted: false, reasonCode: "workspace_hooks_bundle_changed" }}
    : {{ accepted: true }};
  process.stdout.write(JSON.stringify({{ id: message.id, result }}) + "\n");
}});
"#
        ),
    )
    .unwrap();
    let mut env = BTreeMap::new();
    env.insert("ESCODE_PLUGIN_HOST_EXEC_PATH".to_owned(), "node".to_owned());
    env.insert(
        "ESCODE_PLUGIN_HOST_ENTRYPOINT".to_owned(),
        stub.to_string_lossy().into_owned(),
    );

    let surface = start_hooks(&home, &data_base_dir, env).await;
    let mut socket = open(&surface).await;

    let loaded = call_json(
        &mut socket,
        1,
        "loadHooks",
        json!({"workspacePath": workspace}),
    )
    .await
    .into_object()
    .expect("loadHooks answer");
    assert_eq!(loaded["hooksEnabled"], json!(true));
    assert_eq!(loaded["hooks"][0]["command"], json!("node"));
    assert_eq!(
        loaded["hooks"][0]["workspaceHook"]["trustState"],
        json!("pending_trust")
    );
    let digest = loaded["hooks"][0]["workspaceHook"]["hookDeclarationDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = loaded["workspaceHookSnapshot"]["bundleDigest"]
        .as_str()
        .unwrap()
        .to_owned();

    let saved = call_json(
        &mut socket,
        2,
        "saveHooks",
        json!({
            "workspacePath": workspace,
            "hooks": [{
                "id": "from-rpc", "event": "Stop", "type": "process", "command": "node",
                "args": ["stop.mjs"], "enabled": true,
                "location": {"source": "escode", "scope": "project"}
            }]
        }),
    )
    .await;
    assert!(matches!(saved, Wire::Undefined));
    let text = std::fs::read_to_string(workspace.join(".escode/config.json")).unwrap();
    assert!(text.contains("\"Stop\""), "{text}");
    let user_text = std::fs::read_to_string(home.join(".escode/cli/config.json")).unwrap();
    assert!(
        user_text.contains("\"features\""),
        "用户配置保留未知键: {user_text}"
    );

    let granted = call_json(
        &mut socket,
        3,
        "grantWorkspaceHookTrust",
        json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace.to_string_lossy(),
            "bundleDigest": bundle,
            "hookDeclarationDigest": digest,
        }),
    )
    .await
    .into_object()
    .expect("grant answer");
    assert_eq!(granted["accepted"], json!(true), "{granted}");
    let logged = std::fs::read_to_string(&log).unwrap();
    // Host→runtime 是 `workspace/hooks/trustGrant`；runtime→workflow host 是 `hooks.trustGrant`。
    assert!(logged.contains("hooks.trustGrant"), "{logged}");
    assert!(logged.contains(&digest), "{logged}");
    assert!(logged.contains("workspaceKey"), "{logged}");

    let rejected = call_json(
        &mut socket,
        4,
        "grantWorkspaceHookTrust",
        json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace.to_string_lossy(),
            "bundleDigest": bundle,
            "hookDeclarationDigest": "reject",
        }),
    )
    .await
    .into_object()
    .expect("grant rejection is a result");
    assert_eq!(rejected["accepted"], json!(false), "{rejected}");
    assert_eq!(
        rejected["reasonCode"],
        json!("workspace_hooks_bundle_changed")
    );

    // 缺参数：Host 在触达运行时前拒绝。
    let missing = call_json(
        &mut socket,
        5,
        "grantWorkspaceHookTrust",
        json!({"workspacePath": workspace}),
    )
    .await
    .into_object()
    .expect("error payload");
    assert_eq!(missing["name"], json!("InvalidRequest"), "{missing}");

    // 链路语义：hooks 没有推送/流；`/ws`（web-remote-replayable）与
    // `/ws/host`（desktop-continuous）的请求/应答必须一致。
    let capability = issue_capability(&surface).await;
    let mut replayable = connect(surface.addr, "/ws", Some(&capability)).await;
    let replayed = call_json(
        &mut replayable,
        1,
        "loadHooks",
        json!({"workspacePath": workspace}),
    )
    .await
    .into_object()
    .expect("replayable loadHooks answer");
    assert_eq!(replayed["hooksEnabled"], json!(true));
    assert_eq!(replayed["hooks"][0]["type"], json!("process"));
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_workspace_paths_are_rejected_with_an_explicit_error() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data");
    for path in [&home, &data_base_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    let surface = start_hooks(&home, &data_base_dir, BTreeMap::new()).await;
    let mut socket = open(&surface).await;
    let error = call_json(
        &mut socket,
        1,
        "loadHooks",
        json!({"workspacePath": "relative/workspace"}),
    )
    .await
    .into_object()
    .expect("error payload");
    assert_eq!(error["name"], json!("InvalidRequest"), "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("must be absolute"),
        "{error}"
    );
}
