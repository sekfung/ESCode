//! 模型客户端在未解析出代理时直连（与 TS `createNetworkProxyFetch` → 全局 `fetch` 同），
//! 不回落到进程 env 的 `HTTP_PROXY`（docs/specs/rust-net-proxy.md）。
//!
//! 单独成测试二进制：要改进程 env，不能与其他用例并行。
#![allow(clippy::disallowed_methods)] // 测试直接构造 builder 交给 `apply`（与 crate 内单测同）。

use escode_cli_net::{Egress, Purpose, apply};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn the_model_client_ignores_process_proxy_variables_when_none_resolves() {
    // 一个必然连不上的「代理」：若客户端读进程 env，请求会失败。
    // SAFETY: 本测试二进制只有这一个用例，没有其他线程读写进程 env。
    unsafe {
        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
        std::env::set_var("http_proxy", "http://127.0.0.1:9");
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await;
    });
    let direct = Egress::default();
    for purpose in [Purpose::Model, Purpose::Mcp, Purpose::WebFetch] {
        // 每个 purpose 构建都要成功；请求只发一次（服务端只接一个连接）。
        let _ = apply(reqwest::Client::builder(), &direct, purpose)
            .unwrap()
            .build()
            .unwrap();
    }
    let client = apply(reqwest::Client::builder(), &direct, Purpose::Model)
        .unwrap()
        .build()
        .unwrap();
    let body = client
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .expect("direct connection, not the process proxy")
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
}
