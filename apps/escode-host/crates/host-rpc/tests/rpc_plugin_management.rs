//! `plugin-management`：参数整形 + 管理面实例路由（真实内嵌 CLI runtime）。
mod support;
use serde_json::{Value, json};
use support::*;

fn arg(value: Value) -> Wire {
    Wire::Array(vec![Wire::Object(value)])
}

fn object(wire: Wire) -> Value {
    let Wire::Object(value) = wire else {
        panic!("expected an object answer, got {wire:?}")
    };
    value
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_requests_reach_the_management_runtime_with_the_callers_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let (surface, instances) = start_with_instances(&home, &home).await;
    let mut socket = open(&surface).await;
    let ws = workspace.to_string_lossy().into_owned();

    call(
        &mut socket,
        1,
        "plugin-management",
        "listPlugins",
        &arg(json!({"workspacePath": ws, "workspaceIdentity": ws})),
    )
    .await;
    let listed = object(read_response(&mut socket, 1).await);
    assert!(
        listed.get("plugins").is_some(),
        "listPlugins 应答含 plugins: {listed}"
    );

    // 承载进程是固定内部 cwd 的管理面实例，而不是调用方的 workspace。
    let management = home.join(".escode").join("plugin-workspace");
    let management_key = management.to_string_lossy().into_owned();
    assert!(
        instances.live(&management_key).is_some(),
        "管理面实例已启动"
    );
    assert!(instances.live(&ws).is_none(), "没有为 workspace 另起实例");

    call(
        &mut socket,
        2,
        "plugin-management",
        "getPluginsOverview",
        &arg(json!({"workspacePath": ws, "workspaceIdentity": ws})),
    )
    .await;
    let overview = object(read_response(&mut socket, 2).await);
    // 真实 runtime 的应答要通过结果 schema 校验（失败会得到错误帧而不是这些字段）。
    assert!(overview["marketplaces"].is_array(), "{overview}");
    assert!(
        overview["capability"]["supported"].is_boolean(),
        "{overview}"
    );

    // 取消一个不存在的操作：由 runtime 应答（不带 workspace 的请求体）。
    call(
        &mut socket,
        3,
        "plugin-management",
        "cancelPluginOperation",
        &arg(json!({"operationId": "no-such-operation"})),
    )
    .await;
    let cancelled = object(read_response(&mut socket, 3).await);
    assert!(cancelled.is_object(), "{cancelled}");

    // 描述一个不存在的插件：runtime 的错误原样回给调用方（带协议错误名）。
    call(
        &mut socket,
        4,
        "plugin-management",
        "describePlugin",
        &arg(json!({"workspacePath": ws, "workspaceIdentity": ws,
            "marketplace": "ghost-market", "pluginName": "ghost-plugin"})),
    )
    .await;
    let described = read_response(&mut socket, 4).await;
    let text = format!("{described:?}");
    assert!(
        text.contains("ghost") || text.contains("not") || text.contains("找不到"),
        "describe 不存在的插件应得到 runtime 的说明: {text}"
    );

    // 参考目录走 workspace 实例（需要持有 session 记录的进程），不是管理面。
    call(
        &mut socket,
        5,
        "plugin-management",
        "getPluginReferenceCatalog",
        &arg(json!({"workspacePath": ws, "workspaceIdentity": ws})),
    )
    .await;
    let catalog = object(read_response(&mut socket, 5).await);
    assert!(
        matches!(catalog["authority"].as_str(), Some("session" | "workspace")),
        "{catalog}"
    );
    assert!(catalog["plugins"].is_array(), "{catalog}");
    assert!(
        instances.live(&ws).is_some(),
        "目录请求为 workspace 启动了实例"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_data_base_dir_the_channel_reports_an_explicit_gap() {
    let dir = tempfile::tempdir().unwrap();
    let instances = escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded);
    let options: escode_host_surface::OptionsFactory =
        std::sync::Arc::new(|_| panic!("no runtime must start without dataBaseDir"));
    let channels = escode_host_rpc::domains::channels_with(
        dir.path().to_path_buf(),
        None,
        instances,
        options,
        None,
        None,
        None,
    );
    let surface = escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels, None)
        .await
        .expect("surface");
    let mut socket = open(&surface).await;
    call(
        &mut socket,
        1,
        "plugin-management",
        "cancelPluginOperation",
        &arg(json!({"operationId": "x"})),
    )
    .await;
    let answer = format!("{:?}", read_response(&mut socket, 1).await);
    assert!(answer.contains("needs dataBaseDir"), "{answer}");
}
