//! `settings-sync` 端到端（真 WebSocket，`rust-u2-sync.md` §3）。
//!
//! 六个成员：AGENTS.md 迁移、首启提示状态、`detect` 与 `importSelected` 闭环。
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
            Wire::Str("settings-sync".to_owned()),
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
async fn agents_file_migration_and_first_run_state() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join(".claude").join("CLAUDE.md");
    write(&source, "# Claude instructions\n");
    write(
        &home
            .path()
            .join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        "---\nname: alpha\n---\nbody\n",
    );
    let target = home.path().join(".escode").join("AGENTS.md");
    let surface = start(home.path()).await;
    let mut conn = Conn::new(&surface).await;

    let status = conn
        .call(
            1,
            "getClaudeAgentsFileMigrationStatus",
            serde_json::json!({"workspacePath": "/irrelevant"}),
        )
        .await
        .into_object()
        .expect("状态对象");
    assert_eq!(
        status["sourcePath"],
        serde_json::json!(source.to_string_lossy())
    );
    assert_eq!(
        status["targetPath"],
        serde_json::json!(target.to_string_lossy())
    );
    assert_eq!(status["sourceExists"], serde_json::json!(true));
    assert_eq!(status["targetExists"], serde_json::json!(false));
    assert_eq!(status["supported"], serde_json::json!(true));

    let copied = conn
        .call(
            2,
            "copyClaudeAgentsFileToEscodeAgentsFile",
            serde_json::json!({}),
        )
        .await
        .into_object()
        .expect("复制结果");
    assert_eq!(copied["status"], serde_json::json!("copied"));
    assert_eq!(copied["overwritten"], serde_json::json!(false));
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "# Claude instructions\n"
    );

    // 非严格 true 的 overwrite 不覆盖（TS `overwrite !== true`）。
    let skipped = conn
        .call(
            3,
            "copyClaudeAgentsFileToEscodeAgentsFile",
            serde_json::json!({"overwrite": "yes"}),
        )
        .await
        .into_object()
        .expect("跳过结果");
    assert_eq!(skipped["status"], serde_json::json!("skipped"));
    assert_eq!(skipped["skippedReason"], serde_json::json!("targetExists"));

    let state = conn
        .call(4, "getFirstRunPromptState", serde_json::json!({}))
        .await
        .into_object()
        .expect("首启状态");
    assert_eq!(state["handled"], serde_json::json!(false));
    let marked = conn
        .call(5, "markFirstRunPromptHandled", serde_json::json!({}))
        .await;
    assert!(matches!(marked, Wire::Undefined), "{marked:?}");
    let state = conn
        .call(6, "getFirstRunPromptState", serde_json::json!({}))
        .await
        .into_object()
        .expect("首启状态");
    assert_eq!(state["handled"], serde_json::json!(true));

    // detect：只扫描指定的类别，返回 claudeCode 的 skills 候选。
    let detected = conn
        .call(
            7,
            "detect",
            serde_json::json!({"categories": ["skills"], "intent": "manualImport"}),
        )
        .await
        .into_object()
        .expect("发现结果");
    assert_eq!(
        detected["agents"][0]["agent"],
        serde_json::json!("claudeCode")
    );
    let category = &detected["agents"][0]["categories"][0];
    assert_eq!(category["discoveredCount"], serde_json::json!(1));
    assert_eq!(category["importableCount"], serde_json::json!(1));
    assert_eq!(category["selectedByDefault"], serde_json::json!(true));

    let imported = conn
        .call(
            8,
            "importSelected",
            serde_json::json!({
                "selections": [{
                    "agent": "claudeCode", "category": "skills", "importMode": "copy",
                    "skillPaths": [home.path().join(".claude").join("skills").join("alpha").to_string_lossy()],
                }],
            }),
        )
        .await
        .into_object()
        .expect("导入结果");
    assert_eq!(imported["successCount"], serde_json::json!(1));
    assert!(
        home.path()
            .join(".escode")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file()
    );

    let detected = conn
        .call(
            9,
            "detect",
            serde_json::json!({"categories": ["skills"], "intent": "manualImport"}),
        )
        .await
        .into_object()
        .expect("二次发现");
    let category = &detected["agents"][0]["categories"][0];
    assert_eq!(category["importableCount"], serde_json::json!(0));
    assert_eq!(category["skippedCount"], serde_json::json!(1));

    // 没有 manualImport intent 时不扫描。
    let empty = conn
        .call(10, "detect", serde_json::json!({"categories": ["skills"]}))
        .await
        .into_object()
        .expect("空发现结果");
    assert_eq!(empty, serde_json::json!({"agents": []}));

    surface.shutdown().await;
}
