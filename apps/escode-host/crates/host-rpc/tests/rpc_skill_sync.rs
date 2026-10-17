//! `skill-sync` 端到端（真 WebSocket，`rust-u2-sync.md` §1.8）。
//!
//! A 导出 → B 导入 → B 再扫到候选；二次导入 skipped；未知 id / 不安全路径的错误形状。
mod support;
use support::*;

use std::time::Duration;

use base64::Engine as _;

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
            Wire::Str("skill-sync".to_owned()),
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
            let message = tokio::time::timeout(Duration::from_secs(10), self.socket.next())
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

#[tokio::test(flavor = "multi_thread")]
async fn export_from_one_home_imports_into_another() {
    let source_home = tempfile::tempdir().unwrap();
    write(
        &source_home.path().join(".escode/skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: d\n---\nbody\n",
    );
    write(
        &source_home.path().join(".escode/skills/alpha/assets/a.bin"),
        "payload",
    );
    let source_surface = start(source_home.path()).await;
    let mut source = Conn::new(&source_surface).await;

    let listed = source
        .call(1, "listLocalUserSkillCandidates", serde_json::json!({}))
        .await
        .into_object()
        .expect("候选对象");
    assert_eq!(listed["maxArchiveBytes"], serde_json::json!(20_971_520));
    let id = listed["candidates"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert_eq!(
        listed["candidates"][0]["directoryName"],
        serde_json::json!("alpha")
    );

    let exported = source
        .call(
            2,
            "exportSkillsArchive",
            serde_json::json!({"skillIds": [id]}),
        )
        .await
        .into_object()
        .expect("导出对象");
    let encoded = &exported["archive"];
    assert_eq!(
        encoded["__escode_rpc_nested_uint8array_v1"],
        serde_json::json!(true),
        "归档必须走 RPC 嵌套 Uint8Array 标记"
    );
    let archive = base64::engine::general_purpose::STANDARD
        .decode(encoded["base64"].as_str().expect("base64"))
        .expect("archive decodes");
    assert!(exported["archiveBytes"].as_u64().unwrap() > 0);
    assert_eq!(exported["skills"][0]["name"], serde_json::json!("alpha"));

    // 未知 id 的错误形状。
    let error = source
        .call(
            3,
            "exportSkillsArchive",
            serde_json::json!({"skillIds": ["missing"]}),
        )
        .await
        .into_object()
        .expect("错误载荷");
    assert_eq!(error["name"], serde_json::json!("Error"));
    assert_eq!(
        error["message"],
        serde_json::json!("skill sync candidate not found: missing")
    );
    source_surface.shutdown().await;

    // B 导入：嵌套字节 → 落盘；再导入一次全部 skipped。
    let target_home = tempfile::tempdir().unwrap();
    let target_surface = start(target_home.path()).await;
    let mut target = Conn::new(&target_surface).await;
    let payload = serde_json::json!({
        "archive": {
            "__escode_rpc_nested_uint8array_v1": true,
            "base64": base64::engine::general_purpose::STANDARD.encode(&archive),
        },
    });
    let imported = target
        .call(4, "importSkillsArchive", payload.clone())
        .await
        .into_object()
        .expect("导入对象");
    assert_eq!(
        imported["results"][0]["status"],
        serde_json::json!("synced")
    );
    assert_eq!(
        std::fs::read_to_string(target_home.path().join(".escode/skills/alpha/assets/a.bin"))
            .unwrap(),
        "payload"
    );
    let again = target
        .call(5, "importSkillsArchive", payload)
        .await
        .into_object()
        .expect("二次导入");
    assert_eq!(again["results"][0]["status"], serde_json::json!("skipped"));

    let listed = target
        .call(6, "listLocalUserSkillCandidates", serde_json::json!({}))
        .await
        .into_object()
        .expect("候选对象");
    assert_eq!(listed["candidates"].as_array().unwrap().len(), 1);

    let access = target
        .call(7, "checkRemoteUserSkillWriteAccess", serde_json::json!({}))
        .await
        .into_object()
        .expect("写权限对象");
    assert_eq!(access["ok"], serde_json::json!(true));

    let error = target
        .call(
            8,
            "listRemoteUserSkillStatuses",
            serde_json::json!({"directoryNames": ["../escape"]}),
        )
        .await
        .into_object()
        .expect("错误载荷");
    assert_eq!(error["name"], serde_json::json!("Error"));
    assert_eq!(
        error["message"],
        serde_json::json!("unsafe skill sync path: ../escape")
    );

    target_surface.shutdown().await;
}
