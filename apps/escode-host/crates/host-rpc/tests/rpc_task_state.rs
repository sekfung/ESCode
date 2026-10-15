//! `escode-task` 侧栏状态写 + workspace 事件（S3a，docs/specs/rust-u2-tasks-index.md §6.6）端到端：
//! 真 embedded 实例建会话 → `onDynamicWorkspaceEvent` 监听 → pin / unread（含 CAS 清除）/ rename /
//! archive / delete，断言返回的 meta、事件 `reason` 与索引读回。
mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::*;

struct Client {
    socket: Socket,
    frames: FrameReader,
    next: i64,
    events: Vec<Value>,
    listen_id: i64,
}

impl Client {
    async fn send(&mut self, kind: i64, channel: &str, name: &str, arg: Wire) -> i64 {
        self.next += 1;
        let id = self.next;
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str(channel.to_owned()),
            Wire::Str(name.to_owned()),
        ]));
        writer.value(&arg);
        self.socket
            .send(Message::Binary(
                escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
            ))
            .await
            .unwrap();
        id
    }

    async fn frame(&mut self) -> (i64, i64, Wire) {
        loop {
            while let Some(body) = self.frames.next_frame().unwrap() {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().unwrap() else {
                    panic!()
                };
                let kind = header[0].as_int().unwrap();
                if kind == session::RESPONSE_INITIALIZE {
                    continue;
                }
                return (kind, header[1].as_int().unwrap(), reader.value().unwrap());
            }
            let message = tokio::time::timeout(Duration::from_secs(30), self.socket.next())
                .await
                .expect("frame in time")
                .unwrap()
                .unwrap();
            if let Message::Binary(bytes) = message {
                self.frames.push(&bytes);
            }
        }
    }

    /// 调用并等应答；期间收到的 workspace 事件进 `events`。
    async fn call(&mut self, channel: &str, name: &str, params: Value) -> Result<Wire, Value> {
        let id = self
            .send(
                session::REQUEST_PROMISE,
                channel,
                name,
                Wire::Array(vec![Wire::Object(params)]),
            )
            .await;
        loop {
            let (kind, response, data) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE && response == self.listen_id {
                if let Wire::Object(event) = data {
                    self.events.push(event);
                }
                continue;
            }
            if response != id {
                continue;
            }
            return if kind == session::RESPONSE_PROMISE_SUCCESS {
                Ok(data)
            } else {
                Err(data.into_object().unwrap_or(Value::Null))
            };
        }
    }

    /// 等到某个 reason 的事件出现（事件与应答的相对顺序不保证）。
    async fn event(&mut self, reason: &str) -> Value {
        loop {
            if let Some(position) = self
                .events
                .iter()
                .position(|event| event["reason"] == reason)
            {
                return self.events.remove(position);
            }
            let (kind, response, data) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE
                && response == self.listen_id
                && let Wire::Object(event) = data
            {
                self.events.push(event);
            }
        }
    }
}

fn object(wire: Wire) -> Value {
    wire.into_object().expect("object payload")
}

#[tokio::test(flavor = "multi_thread")]
async fn sidebar_state_writes_update_the_index_and_broadcast_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let data_base_dir = dir.path().join("data-base");
    let workspace_dir = dir.path().join("workspace");
    for path in [&home, &data_base_dir, &workspace_dir] {
        std::fs::create_dir_all(path).unwrap();
    }
    support::prepare_index(&data_base_dir);
    let workspace = workspace_dir.to_string_lossy().into_owned();
    let model_config = dir.path().join("model.json");
    std::fs::write(
        &model_config,
        serde_json::to_vec(&json!({"providerId": "fixture", "modelId": "fixture-model",
            "reasoningLevel": "none", "baseUrl": "http://127.0.0.1:9/v1"}))
        .unwrap(),
    )
    .unwrap();
    let surface = start_with_data_base_dir(&home, &data_base_dir).await;
    let mut client = Client {
        socket: open(&surface).await,
        frames: FrameReader::new(),
        next: 0,
        events: Vec::new(),
        listen_id: 0,
    };
    // 监听 workspace 事件（只登记，不启动 runtime）。
    client.listen_id = client
        .send(
            session::REQUEST_EVENT_LISTEN,
            "escode-task",
            "onDynamicWorkspaceEvent",
            Wire::Object(json!({"workspacePath": workspace, "workspaceIdentity": workspace})),
        )
        .await;

    // 真会话 → 索引行（createSession 的空白快照只写索引不广播）。
    let snapshot = object(
        client
            .call(
                "escode-session",
                "createSession",
                json!({"workspacePath": workspace, "workspaceIdentity": workspace,
                    "config": model_config.to_string_lossy(), "mode": "build",
                    "model": {"providerId": "fixture", "modelId": "fixture-model", "options": {"reasoningLevel": "none"}},
                    "persistence": "immediate"}),
            )
            .await
            .expect("createSession"),
    );
    let task_id = snapshot["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = |extra: Value| {
        let mut params =
            json!({"workspacePath": workspace, "workspaceIdentity": workspace, "taskId": task_id});
        if let Value::Object(extra) = extra {
            params.as_object_mut().unwrap().extend(extra);
        }
        params
    };

    // pin → task_pinned；unpin → task_unpinned。
    let pinned = object(
        client
            .call(
                "escode-task",
                "setTaskPinned",
                task(json!({"pinned": true})),
            )
            .await
            .unwrap(),
    );
    assert_eq!(pinned["taskId"], Value::from(task_id.clone()));
    let event = client.event("task_pinned").await;
    assert_eq!(event["type"], "workspace_task_list_changed");
    assert_eq!(event["taskId"], Value::from(task_id.clone()));
    assert_eq!(event["workspaceIdentity"], Value::from(workspace.clone()));
    let Wire::Array(pinned_list) = client
        .call(
            "escode-task",
            "listPinnedTasks",
            json!({"workspacePath": workspace, "workspaceIdentity": workspace}),
        )
        .await
        .unwrap()
    else {
        panic!("pinned list")
    };
    assert_eq!(pinned_list.len(), 1);
    client
        .call(
            "escode-task",
            "setTaskPinned",
            task(json!({"pinned": false})),
        )
        .await
        .unwrap();
    client.event("task_unpinned").await;

    // unread：分配标记 → CAS 旧值不清、当前值清。
    let unread = object(
        client
            .call(
                "escode-task",
                "setTaskUnread",
                task(json!({"unread": true})),
            )
            .await
            .unwrap(),
    );
    let marker = unread["unreadAt"].as_i64().expect("unreadAt");
    assert_eq!(
        client.event("task_meta_changed").await["taskMeta"]["unreadAt"],
        marker
    );
    let stale = object(
        client
            .call(
                "escode-task",
                "setTaskUnread",
                task(json!({"unread": false, "expectedUnreadAt": marker - 1})),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        stale["unreadAt"], marker,
        "stale CAS keeps the newer unread"
    );
    let cleared = object(
        client
            .call(
                "escode-task",
                "setTaskUnread",
                task(json!({"unread": false, "expectedUnreadAt": marker})),
            )
            .await
            .unwrap(),
    );
    assert!(cleared.get("unreadAt").is_none());

    // rename → 手动标题 + task_title_changed；V4 session store 同步（运行时有该会话）。
    let renamed = object(
        client
            .call(
                "escode-task",
                "renameTask",
                task(json!({"title": "手动标题"})),
            )
            .await
            .unwrap(),
    );
    assert_eq!(renamed["title"], "手动标题");
    assert_eq!(renamed["titleOverridden"], true);
    assert_eq!(
        client.event("task_title_changed").await["taskMeta"]["title"],
        "手动标题"
    );
    let read = object(
        client
            .call("escode-session", "readSession", json!({"workspacePath": workspace, "workspaceIdentity": workspace, "sessionId": task_id}))
            .await
            .unwrap(),
    );
    assert_eq!(
        read["session"]["title"], "手动标题",
        "renameSession reached the runtime store"
    );

    // archive / unarchive。
    client
        .call("escode-task", "archiveTask", task(json!({})))
        .await
        .unwrap();
    client.event("task_archived").await;
    client
        .call("escode-task", "unarchiveTask", task(json!({})))
        .await
        .unwrap();
    client.event("task_unarchived").await;

    // 删归档：未归档 → false；deleteTask → task_deleted，索引读不到。
    let skipped = client
        .call("escode-task", "deleteArchivedTask", task(json!({})))
        .await
        .unwrap();
    assert_eq!(skipped, Wire::Object(Value::Bool(false)));
    let deleted = client
        .call("escode-task", "deleteTask", task(json!({})))
        .await
        .unwrap();
    assert_eq!(deleted, Wire::Undefined);
    assert_eq!(
        client.event("task_deleted").await["taskId"],
        Value::from(task_id.clone())
    );
    let meta = client
        .call("escode-task", "getTaskMeta", task(json!({})))
        .await
        .unwrap();
    assert_eq!(meta, Wire::Undefined, "deleted task is gone from the index");
    // 已删除再写：TS 同文错误。
    let error = client
        .call(
            "escode-task",
            "setTaskPinned",
            task(json!({"pinned": true})),
        )
        .await
        .expect_err("deleted");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("task index 中不存在 task"),
        "{error}"
    );
    surface.shutdown().await;
}
