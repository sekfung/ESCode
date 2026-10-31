use super::*;
use escode_host_services::source_server::SourceServerRegistry;
use escode_host_services::source_server_connections::ConnectionsOptions;
use escode_host_services::source_server_exec::ResolvedTransport;

fn connections() -> Arc<SourceServerConnections> {
    let options = ConnectionsOptions::new("test", |_profile| {
        Box::pin(async { Err::<ResolvedTransport, _>(anyhow::anyhow!("unused")) })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = anyhow::Result<ResolvedTransport>> + Send>,
            >
    });
    Arc::new(SourceServerConnections::new(
        Arc::new(SourceServerRegistry::empty()),
        options,
    ))
}

/// 远端 hostId 没有在线 Source 时显式失败，不落本机库。
#[tokio::test]
async fn remote_host_without_connection_fails_explicitly() {
    let channel = project_channel(
        ReadContext::unavailable(),
        PathBuf::from("/tmp"),
        None,
        connections(),
    );
    let handler = channel.handler("readDirectory").expect("method");
    let error = handler(Wire::Array(vec![Wire::Object(json!({
        "hostId": "remote:server:abc",
        "path": "/srv"
    }))]))
    .await
    .expect_err("must fail");
    assert_eq!(error.message, "project.host-offline");
}

/// 本机 hostId 仍走本地实现（这里用未装配任务库验证没有误路由）。
#[tokio::test]
async fn local_host_stays_on_local_implementation() {
    let channel = project_channel(
        ReadContext::unavailable(),
        PathBuf::from("/tmp"),
        None,
        connections(),
    );
    let handler = channel.handler("readDirectory").expect("method");
    // 本地实现会真正读目录；只要没有返回 `project.host-offline`，就说明没有误路由到
    // 不存在的远程 Source。
    let result = handler(Wire::Array(vec![Wire::Object(json!({
        "hostId": "local",
        "path": "/tmp"
    }))]))
    .await;
    match result {
        Ok(Wire::Object(value)) => assert_eq!(value["path"], json!("/tmp")),
        Ok(_) => panic!("unexpected wire shape"),
        Err(error) => assert_ne!(error.message, "project.host-offline"),
    }
}
