//! `mcp-sync` 端到端（真 WebSocket，`rust-u2-sync.md` §2）。
//!
//! A 导出 → B 导入（filesystem 参数按远端根改写）→ 二次导入 skipped；
//! 配置写入 / 状态查询 / 写权限探测 / 运行态 `mcp/list` 转发。
mod support;
use support::*;

use std::time::Duration;

struct Conn {
    socket: Socket,
    frames: FrameReader,
}

impl Conn {
    async fn new(surface: &RpcSurface) -> Self {
        Self {
            socket: open(surface).await,
            frames: FrameReader::new(),
        }
    }

    async fn call(&mut self, id: i64, method: &str, params: serde_json::Value) -> Wire {
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(session::REQUEST_PROMISE),
            Wire::Int(id),
            Wire::Str("mcp-sync".to_owned()),
            Wire::Str(method.to_owned()),
        ]));
        writer.value(&Wire::Array(vec![Wire::Object(params)]));
        self.socket
            .send(Message::Binary(
                escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
            ))
            .await
            .expect("frame sent");
        loop {
            while let Some(body) = self.frames.next_frame().expect("frame parses") {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().expect("header") else {
                    panic!("response header must be an array")
                };
                if header[0].as_int() == Some(session::RESPONSE_INITIALIZE) {
                    continue;
                }
                if header[1].as_int() == Some(id) {
                    return reader.value().expect("payload");
                }
            }
            let message = tokio::time::timeout(Duration::from_secs(30), self.socket.next())
                .await
                .expect("frame within timeout")
                .expect("socket open")
                .expect("frame ok");
            match message {
                Message::Binary(bytes) => self.frames.push(&bytes),
                Message::Text(text) => self.frames.push(text.as_bytes()),
                _ => continue,
            }
        }
    }
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn export_import_and_config_writes_round_trip() {
    let source_home = tempfile::tempdir().unwrap();
    write(
        &source_home.path().join(".escode/cli/config.json"),
        &serde_json::json!({
            "mcp": {"servers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem",
                             source_home.path().join("project").to_string_lossy()],
                },
            }},
            "keep": true,
        })
        .to_string(),
    );
    let source_surface = start(source_home.path()).await;
    let mut source = Conn::new(&source_surface).await;

    let candidates = source
        .call(1, "listLocalUserMcpCandidates", serde_json::json!({}))
        .await
        .into_object()
        .expect("候选对象");
    let id = candidates["candidates"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let exported = source
        .call(
            2,
            "exportMcpServers",
            serde_json::json!({"serverIds": [id]}),
        )
        .await
        .into_object()
        .expect("导出对象");
    assert_eq!(
        exported["servers"][0]["name"],
        serde_json::json!("filesystem")
    );

    // 写面：upsert 保留其它顶层字段。
    source
        .call(
            3,
            "saveMcpToUserDirectory",
            serde_json::json!({
                "action": "upsert", "name": "added",
                "config": {"url": "https://example.test/mcp"},
            }),
        )
        .await;
    let config = read(&source_home.path().join(".escode/cli/config.json"));
    assert_eq!(config["keep"], serde_json::json!(true));
    assert_eq!(
        config["mcp"]["servers"]["added"]["url"],
        serde_json::json!("https://example.test/mcp")
    );
    let loaded = source
        .call(4, "loadMcpFromUserDirectory", serde_json::json!({}))
        .await
        .into_object()
        .expect("load 对象");
    assert_eq!(loaded["servers"].as_array().unwrap().len(), 2);
    source_surface.shutdown().await;

    let target_home = tempfile::tempdir().unwrap();
    let target_workspace = target_home.path().join("project");
    std::fs::create_dir_all(&target_workspace).unwrap();
    let target_surface = start(target_home.path()).await;
    let mut target = Conn::new(&target_surface).await;
    let imported = target
        .call(
            5,
            "importMcpServers",
            serde_json::json!({
                "servers": [exported["servers"][0].clone()],
                "localHomeDir": source_home.path().to_string_lossy(),
                "localWorkspacePath": source_home.path().join("project").to_string_lossy(),
                "remoteWorkspacePath": target_workspace.to_string_lossy(),
            }),
        )
        .await
        .into_object()
        .expect("导入对象");
    assert_eq!(
        imported["results"][0]["status"],
        serde_json::json!("synced")
    );
    let config = read(&target_home.path().join(".escode/cli/config.json"));
    assert_eq!(
        config["mcp"]["servers"]["filesystem"]["args"][2],
        serde_json::json!(target_workspace.to_string_lossy())
    );

    let again = target
        .call(
            6,
            "importMcpServers",
            serde_json::json!({"servers": [exported["servers"][0].clone()]}),
        )
        .await
        .into_object()
        .expect("二次导入");
    assert_eq!(again["results"][0]["status"], serde_json::json!("skipped"));

    let statuses = target
        .call(
            7,
            "listRemoteUserMcpStatuses",
            serde_json::json!({"names": ["filesystem"]}),
        )
        .await
        .into_object()
        .expect("状态对象");
    assert_eq!(statuses["statuses"][0]["exists"], serde_json::json!(true));
    let access = target
        .call(8, "checkRemoteUserMcpWriteAccess", serde_json::json!({}))
        .await
        .into_object()
        .expect("写权限对象");
    assert_eq!(access["ok"], serde_json::json!(true));

    // 运行态查询：转发 workspace 实例的 `mcp/list`（空 server 集，status-only）。
    let status = target
        .call(
            9,
            "listWorkspaceMcpServerStatuses",
            serde_json::json!({
                "workspacePath": target_workspace.to_string_lossy(),
                "mcpServers": [],
                "mode": "status",
            }),
        )
        .await;
    let Wire::Object(status) = status else {
        panic!("mcp/list 应答应为对象: {status:?}");
    };
    assert!(
        status.get("statuses").is_some(),
        "mcp/list 结果应含 statuses: {status}"
    );
    target_surface.shutdown().await;
}
