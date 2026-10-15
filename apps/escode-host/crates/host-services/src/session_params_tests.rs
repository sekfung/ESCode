//! `session_params` 的语义边界用例（对齐 `buildSessionCreateParams` / `buildSessionResumeParams`
//! / `appendWorkspaceToFilesystemMcpServers` 的每条规则）。

use serde_json::{Value, json};

use super::{
    ToolGates, append_workspace_to_filesystem_mcp_servers, replayable_thought_level,
    session_create_params, session_forward_params, session_resume_params, with_api_retry_runtime,
};

fn closed() -> ToolGates {
    ToolGates::default()
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys = value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

#[test]
fn create_params_keep_the_typescript_whitelist() {
    let params = json!({
        "workspacePath": "/w",
        "workspaceIdentity": "/w",
        "sessionId": "sess_1",
        "parentSessionId": "sess_parent",
        "mode": "plan",
        "model": {"providerId": "fixture", "modelId": "fixture-model"},
        "persistence": "deferred",
        "thoughtLevel": "high",
        "titleGenerationEnabled": false,
        "mcpServers": [{"name": "filesystem"}],
        "toolAllowlist": ["Read"],
        "toolDenylist": ["Bash"],
        "importedHistory": {"source": "claudeCode", "messages": []},
        "sessionTraceId": "trace-1",
        "deliveryKind": "desktop-continuous",
    });
    let request = session_create_params(&params, &closed());
    assert_eq!(
        keys(&request),
        vec![
            "importedHistory",
            "mcpServers",
            "mode",
            "model",
            "parentSessionId",
            "persistence",
            "sessionId",
            "thoughtLevel",
            "titleGenerationEnabled",
            "toolAllowlist",
            "toolDenylist",
        ],
        "白名单外的键（含 sessionTraceId/deliveryKind/workspace 路由键）不进运行时参数"
    );
    assert_eq!(request["sessionId"], json!("sess_1"));
    assert_eq!(request["parentSessionId"], json!("sess_parent"));
    assert_eq!(request["persistence"], json!("deferred"));
    // 灰度 flag 关闭/缺省时整键不写。
    assert!(request.get("offPeakToolEnabled").is_none());
    assert!(request.get("dynamicWorkflowEnabled").is_none());
    assert!(request.get("dynamicWorkflowMode").is_none());
    assert!(request.get("titleGenerationEnabled").is_some());
}

#[test]
fn absent_and_null_optional_fields_are_omitted() {
    let request = session_create_params(
        &json!({
        "sessionId": "sess_1",
        "model": null,
        "mcpServers": null,
        "thoughtLevel": "",
        }),
        &closed(),
    );
    // `thoughtLevel: ""` 是「有值」（TS 的 `!== undefined` 判定），其余缺省/null 整键消失。
    assert_eq!(keys(&request), vec!["sessionId", "thoughtLevel"]);
    assert_eq!(request["thoughtLevel"], json!(""));
    assert!(request.get("model").is_none());
    assert!(request.get("mcpServers").is_none());
    assert!(request.get("persistence").is_none());
}

#[test]
fn grayscale_flags_come_from_host_gates_never_from_renderer_params() {
    // Renderer 入参里的灰度字段一律忽略（TS 用 Host 判定值覆盖入参）。
    let forged = json!({
        "sessionId": "sess_1",
        "offPeakToolEnabled": true,
        "dynamicWorkflowEnabled": true,
        "dynamicWorkflowMode": "alwaysOn",
    });
    for request in [
        session_create_params(&forged, &closed()),
        session_resume_params(&forged, &closed()),
    ] {
        assert!(request.get("offPeakToolEnabled").is_none());
        assert!(request.get("dynamicWorkflowEnabled").is_none());
        assert!(request.get("dynamicWorkflowMode").is_none());
    }
    // Host 判定开启时：只在命中时下发 true，mode 与布尔同行。
    let open = ToolGates {
        off_peak: true,
        dynamic_workflow_mode: Some("alwaysOn".to_owned()),
    };
    for request in [
        session_create_params(&json!({"sessionId": "sess_1"}), &open),
        session_resume_params(&json!({"sessionId": "sess_1"}), &open),
    ] {
        assert_eq!(request["offPeakToolEnabled"], json!(true));
        assert_eq!(request["dynamicWorkflowEnabled"], json!(true));
        assert_eq!(request["dynamicWorkflowMode"], json!("alwaysOn"));
    }
    let off_peak_only = session_create_params(
        &json!({"sessionId": "sess_1"}),
        &ToolGates {
            off_peak: true,
            dynamic_workflow_mode: None,
        },
    );
    assert!(off_peak_only.get("dynamicWorkflowEnabled").is_none());
    assert!(off_peak_only.get("dynamicWorkflowMode").is_none());
}

#[test]
fn resume_params_have_no_create_only_fields() {
    let request = session_resume_params(
        &json!({
        "sessionId": "sess_1",
        "workspacePath": "/w",
        "model": {"providerId": "fixture", "modelId": "fixture-model"},
        "thoughtLevel": "none",
        "mcpServers": [{"name": "mcp1"}],
        "toolDenylist": ["Bash"],
        "persistence": "immediate",
        "parentSessionId": "sess_parent",
        "importedHistory": {"source": "claudeCode"},
        }),
        &closed(),
    );
    assert_eq!(
        keys(&request),
        vec!["mcpServers", "sessionId", "thoughtLevel", "toolDenylist"],
        "resume 不带 model/persistence/parentSessionId/importedHistory/deliveryKind"
    );
}

#[test]
fn filesystem_mcp_gains_the_workspace_only_when_the_path_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().to_string_lossy().into_owned();
    let servers = json!([
        {"name": "filesystem", "command": "npx",
         "args": ["-y", "@modelcontextprotocol/server-filesystem", "/other"]},
    ]);
    let updated =
        append_workspace_to_filesystem_mcp_servers(Some(&servers), &workspace).expect("servers");
    let args = updated[0]["args"].as_array().expect("args");
    assert_eq!(
        args.last().and_then(Value::as_str),
        Some(workspace.as_str())
    );
    // 幂等：已经包含当前 workspace 时不再追加。
    let again =
        append_workspace_to_filesystem_mcp_servers(Some(&updated), &workspace).expect("servers");
    assert_eq!(again, updated);
    // 尾部分隔符差异按同一路径处理（TS `normalizePathForCompare`）。
    let with_slash = format!("{workspace}/");
    let trailing =
        append_workspace_to_filesystem_mcp_servers(Some(&updated), &with_slash).expect("servers");
    assert_eq!(trailing, updated);
    // workspace 不存在：原样返回（远程/未建目录不注入本机路径）。
    let missing = append_workspace_to_filesystem_mcp_servers(
        Some(&servers),
        &format!("{workspace}/definitely-missing"),
    )
    .expect("servers");
    assert_eq!(missing, servers);
}

#[test]
fn non_filesystem_mcp_servers_are_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().to_string_lossy().into_owned();
    let servers = json!([
        {"name": "chrome-devtools", "command": "npx", "args": ["-y", "chrome-devtools-mcp"]},
        {"name": "filesystem", "command": "npx", "args": ["-y", "other-server"]},
        {"name": "filesystem", "url": "https://example.com/mcp"},
    ]);
    let updated =
        append_workspace_to_filesystem_mcp_servers(Some(&servers), &workspace).expect("servers");
    assert_eq!(
        updated, servers,
        "只有官方 filesystem 包（command + 包名）才注入"
    );
    // 无 servers：保持 undefined；空数组原样返回。
    assert!(append_workspace_to_filesystem_mcp_servers(None, &workspace).is_none());
    assert!(append_workspace_to_filesystem_mcp_servers(Some(&Value::Null), &workspace).is_none());
    assert_eq!(
        append_workspace_to_filesystem_mcp_servers(Some(&json!([])), &workspace),
        Some(json!([]))
    );
}

#[test]
fn forwarded_methods_send_exactly_the_typescript_literals() {
    let params = json!({
        "workspacePath": "/w", "workspaceIdentity": "/w", "remoteSessionId": "r",
        "sessionId": "s", "model": {"providerId": "p", "modelId": "m"}, "thoughtLevel": "high",
        "mode": "plan", "expectedRevision": 3, "persistAsWorkspaceLastUsed": true,
        "deliveryKind": "desktop-continuous", "messageLimit": 20, "afterSeq": 4,
        "contentProfile": "full", "afterMessageId": "m1", "limit": 5, "includeArchived": true,
        "sessionIds": ["s"], "expectedPersistence": "deferred", "forged": 1,
    });
    let cases = [
        (
            "listSessions",
            "session/list",
            vec!["includeArchived", "limit", "sessionIds"],
        ),
        (
            "readSession",
            "session/read",
            vec![
                "afterSeq",
                "contentProfile",
                "deliveryKind",
                "messageLimit",
                "sessionId",
            ],
        ),
        (
            "readSessionMessages",
            "session/messages",
            vec!["afterMessageId", "limit", "sessionId"],
        ),
        (
            "closeSession",
            "session/close",
            vec!["expectedPersistence", "sessionId"],
        ),
        (
            "setModel",
            "session/setModel",
            vec![
                "expectedRevision",
                "model",
                "persistAsWorkspaceLastUsed",
                "sessionId",
            ],
        ),
        (
            "setThoughtLevel",
            "session/setThoughtLevel",
            vec![
                "expectedRevision",
                "persistAsWorkspaceLastUsed",
                "sessionId",
                "thoughtLevel",
            ],
        ),
        (
            "setMode",
            "session/setMode",
            vec!["expectedRevision", "mode", "sessionId"],
        ),
    ];
    for (method, runtime_method, expected) in cases {
        let (resolved, request) = session_forward_params(method, &params).expect("known method");
        assert_eq!(resolved, runtime_method);
        assert_eq!(keys(&request), expected, "{method}");
    }
    // includeArchived 恒在场（缺省 false）；expectedPersistence 只在真值时带。
    let (_, list) = session_forward_params("listSessions", &json!({})).unwrap();
    assert_eq!(list, json!({"includeArchived": false}));
    let (_, close) = session_forward_params(
        "closeSession",
        &json!({"sessionId": "s", "expectedPersistence": ""}),
    )
    .unwrap();
    assert_eq!(close, json!({"sessionId": "s"}));
    assert!(session_forward_params("createSession", &params).is_none());
}

#[test]
fn api_retry_runtime_only_fills_terminal_snapshots() {
    let snapshot = |status: &str, runtime: Value| json!({"session": {"sessionId": "s", "status": status, "workspace": {"workspacePath": "/w"}}, "runtime": runtime});
    assert_eq!(
        with_api_retry_runtime(snapshot("completed", json!({"stateRevision": 1})))["runtime"],
        json!({"stateRevision": 1, "apiRetry": null})
    );
    assert_eq!(
        with_api_retry_runtime(snapshot("error", json!({})))["runtime"],
        json!({"apiRetry": null})
    );
    // 运行中：无跟踪状态时原样（有状态部分随事件面接入）。
    assert_eq!(
        with_api_retry_runtime(snapshot("running", json!({"stateRevision": 1})))["runtime"],
        json!({"stateRevision": 1})
    );
    // 已带 apiRetry：原样。
    let retrying = json!({"attempt": 2});
    assert_eq!(
        with_api_retry_runtime(snapshot("completed", json!({"apiRetry": retrying.clone()})))["runtime"]
            ["apiRetry"],
        retrying
    );
    // 缺身份（sessionId/workspacePath）：原样。
    let anonymous = json!({"session": {"status": "completed"}, "runtime": {}});
    assert_eq!(with_api_retry_runtime(anonymous.clone()), anonymous);
}

fn thought_snapshot(current: &str, enabled: bool, available: Value) -> Value {
    json!({"settings": {"thoughtLevel": {"current": current, "enabled": enabled, "available": available}}})
}

#[test]
fn thought_level_replay_follows_the_snapshot_capabilities() {
    let params = json!({"thoughtLevel": " max "});
    // 档位可用：重放。
    assert_eq!(
        replayable_thought_level(
            &params,
            &thought_snapshot("high", true, json!(["high", {"value": "max"}]))
        ),
        Some("max".to_owned())
    );
    // 档位不在可用列表：放弃。
    assert_eq!(
        replayable_thought_level(
            &params,
            &thought_snapshot("high", true, json!(["low", "high"]))
        ),
        None
    );
    // 可用列表为空：不限制。
    assert_eq!(
        replayable_thought_level(&params, &thought_snapshot("high", true, json!([]))),
        Some("max".to_owned())
    );
    // 思考关闭（enabled=false）：空集，放弃。
    assert_eq!(
        replayable_thought_level(&params, &thought_snapshot("high", false, json!(["max"]))),
        None
    );
    // 已一致：保留（用作索引 override，不再重放）。
    assert_eq!(
        replayable_thought_level(&params, &thought_snapshot("max", false, json!([]))),
        Some("max".to_owned())
    );
    assert_eq!(
        replayable_thought_level(
            &json!({"thoughtLevel": "  "}),
            &thought_snapshot("high", true, json!([]))
        ),
        None
    );
}
