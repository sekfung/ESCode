//! `escode-task` 批量归档 / 原生日志（`rust-u2-tasks-index.md` §9）。
//!
//! 夹具直接建 tasks-index（与 TS Host 同 schema/账本），再经真 WebSocket 调用两个归档方法与日志路径。
mod support;
use support::*;

use std::time::Duration;

use escode_host_services::tasks_index::{SyncOptions, TasksIndex, schema};
use serde_json::json;

struct Conn {
    socket: Socket,
    frames: FrameReader,
    events: Vec<Wire>,
}

impl Conn {
    async fn new(surface: &RpcSurface) -> Self {
        Self {
            socket: open(surface).await,
            frames: FrameReader::new(),
            events: Vec::new(),
        }
    }

    async fn send(&mut self, kind: i64, id: i64, method: &str, arg: &Wire) {
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str("escode-task".to_owned()),
            Wire::Str(method.to_owned()),
        ]));
        writer.value(arg);
        self.socket
            .send(Message::Binary(
                escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
            ))
            .await
            .expect("frame sent");
    }

    async fn frame(&mut self) -> (i64, i64, Wire) {
        loop {
            while let Some(body) = self.frames.next_frame().expect("frame parses") {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().expect("header") else {
                    panic!("response header must be an array")
                };
                let kind = header[0].as_int().expect("kind");
                if kind == session::RESPONSE_INITIALIZE {
                    continue;
                }
                return (
                    kind,
                    header[1].as_int().unwrap_or_default(),
                    reader.value().expect("payload"),
                );
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

    async fn call(&mut self, id: i64, method: &str, params: serde_json::Value) -> Wire {
        self.send(
            session::REQUEST_PROMISE,
            id,
            method,
            &Wire::Array(vec![Wire::Object(params)]),
        )
        .await;
        loop {
            let (kind, response_id, payload) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE {
                self.events.push(payload);
                continue;
            }
            if response_id == id {
                return payload;
            }
        }
    }

    async fn next_event(&mut self) -> Wire {
        if !self.events.is_empty() {
            return self.events.remove(0);
        }
        loop {
            let (kind, _, payload) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE {
                return payload;
            }
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn meta(
    task_id: &str,
    updated_at: i64,
    status: &str,
    provider: &str,
) -> serde_json::Map<String, serde_json::Value> {
    json!({
        "taskId": task_id,
        "traceId": format!("escode-{task_id}"),
        "title": format!("标题 {task_id}"),
        "workspacePath": "/w",
        "createdAt": 1,
        "updatedAt": updated_at,
        "mode": "build",
        "status": status,
        "provider": provider,
    })
    .as_object()
    .unwrap()
    .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn archives_stale_and_workspace_tasks_and_reports_the_native_log() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let db_path = data
        .path()
        .join(".escode")
        .join("v2")
        .join("tasks-index.sqlite");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    {
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        escode_host_services::tasks_index::fixture::apply_generated_schema(
            &connection,
            schema::EXPECTED_MIGRATIONS,
        )
        .unwrap();
    }
    let old = now_ms() - 10 * 24 * 60 * 60 * 1000;
    let recent = now_ms() - 60 * 60 * 1000;
    {
        let index = TasksIndex::open(&db_path).unwrap();
        for (task_id, updated_at, status, provider) in [
            ("stale", old, "completed", "glm"),
            ("recent", recent, "completed", "glm"),
            ("running", old, "running", "glm"),
            ("other", old, "completed", "claude"),
        ] {
            index
                .sync_task_meta(
                    &meta(task_id, updated_at, status, provider),
                    &SyncOptions::default(),
                )
                .unwrap();
        }
    }

    let surface = start_with_data_base_dir(home.path(), data.path()).await;
    let mut conn = Conn::new(&surface).await;

    let stale = conn
        .call(
            1,
            "archiveStaleTasks",
            json!({"workspacePath": "/w", "olderThanDays": 7}),
        )
        .await;
    let Wire::Array(stale) = stale else {
        panic!("archiveStaleTasks 应答应为数组: {stale:?}");
    };
    let ids: Vec<String> = stale
        .iter()
        .filter_map(|item| {
            item.clone()
                .into_object()
                .and_then(|meta| meta["taskId"].as_str().map(str::to_owned))
        })
        .collect();
    assert_eq!(
        ids,
        vec!["stale", "other"],
        "不带 provider 时清理全部历史 provider"
    );

    // 事件订阅：workspace 级归档广播一次（不带 taskId）。
    conn.send(
        session::REQUEST_EVENT_LISTEN,
        50,
        "onDynamicWorkspaceEvent",
        &Wire::Object(json!({"workspacePath": "/w"})),
    )
    .await;
    let archived = conn
        .call(2, "archiveWorkspaceTasks", json!({"workspacePath": "/w"}))
        .await;
    let Wire::Array(archived) = archived else {
        panic!("archiveWorkspaceTasks 应答应为数组: {archived:?}");
    };
    let mut archived_ids: Vec<String> = archived
        .iter()
        .filter_map(|item| {
            item.clone()
                .into_object()
                .and_then(|meta| meta["taskId"].as_str().map(str::to_owned))
        })
        .collect();
    archived_ids.sort();
    assert_eq!(archived_ids, vec!["recent", "running", "stale"]);
    let Wire::Object(event) = conn.next_event().await else {
        panic!("workspace 事件应为对象");
    };
    assert_eq!(event["reason"], json!("task_meta_changed"));
    assert!(event.get("taskId").is_none());

    let log = conn
        .call(3, "getTaskNativeSessionLogFile", json!({}))
        .await
        .into_object()
        .expect("日志对象");
    assert_eq!(log["provider"], json!("glm"));
    let path = log["path"].as_str().unwrap();
    assert!(path.contains("escode-rust-"), "{path}");
    assert!(path.ends_with(".jsonl"), "{path}");
    assert!(log["exists"].is_boolean());

    surface.shutdown().await;
}
