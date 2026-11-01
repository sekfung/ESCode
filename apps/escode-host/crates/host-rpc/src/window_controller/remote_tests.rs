//! 远端 window-controller 单测（spec §10.13 验收 1–5）。
//!
//! 不启动真实 transport：用 `RemoteChannelClient` 的线帧接口模拟 `escode-task`，验证
//! 读取合并、写面转发、作用域隔离、缺失 channel 的显式失败和搜索失败降级为空。

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::source_server_client::{
    RESPONSE_INITIALIZE, RESPONSE_PROMISE_ERROR, RESPONSE_PROMISE_SUCCESS, RemoteChannelClient,
};

use super::*;

const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
const OTHER_SOURCE: &str = "55555555-5555-4555-8555-555555555555";
const PROFILE: &str = "44444444-4444-4444-8444-444444444444";
const IDENTITY: &str = "remote:server:33333333-3333-4333-8333-333333333333:/remote/workspace";
const TASK_CHANNEL: &str = "escode-task";

fn scope(identity: &str) -> crate::remote_scope::RemoteScope {
    crate::remote_scope::RemoteScope {
        source_server_id: SOURCE.to_owned(),
        profile_id: PROFILE.to_owned(),
        workspace_identity: identity.to_owned(),
    }
}

/// 已 `Initialize`（广播 `channels`）的客户端 + 出站帧接收端。
fn initialized_client(
    channels: &[&str],
) -> (Arc<RemoteChannelClient>, mpsc::UnboundedReceiver<Vec<u8>>) {
    let (outbound, receiver) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    let mut payload = WireValue::Array(vec![WireValue::Int(RESPONSE_INITIALIZE)]).encode();
    WireValue::Object(json!({ "channels": channels })).encode_into(&mut payload);
    client.handle_frame(&payload).expect("initialize frame");
    (client, receiver)
}

#[derive(Clone)]
struct FakeTask {
    task_id: String,
    title: String,
    updated_at: i64,
    pinned: bool,
    archived: bool,
    unread: bool,
}

impl FakeTask {
    fn new(task_id: &str, title: &str, updated_at: i64) -> Self {
        Self {
            task_id: task_id.to_owned(),
            title: title.to_owned(),
            updated_at,
            pinned: false,
            archived: false,
            unread: true,
        }
    }

    fn meta(&self) -> Value {
        json!({
            "taskId": self.task_id,
            "title": self.title,
            "workspacePath": "/remote/workspace",
            "workspaceIdentity": IDENTITY,
            "createdAt": self.updated_at,
            "updatedAt": self.updated_at,
            "mode": "agent",
            "status": "completed",
            "unread": self.unread,
        })
    }
}

#[derive(Default, Clone)]
struct Store {
    tasks: Arc<Mutex<Vec<FakeTask>>>,
    /// 非空时 `listTaskList` 返回错误，用于验证搜索失败降级为空。
    fail_search: Arc<Mutex<bool>>,
    /// 最近一次 `listTaskList` 的 workspaceScopes，用于验证 Host 把查询收敛到本连接 scope。
    last_search_scopes: Arc<Mutex<Option<Value>>>,
}

impl Store {
    fn with(tasks: Vec<FakeTask>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(tasks)),
            fail_search: Arc::new(Mutex::new(false)),
            last_search_scopes: Arc::new(Mutex::new(None)),
        }
    }

    fn task(&self, task_id: &str) -> Option<FakeTask> {
        let tasks = self.tasks.lock().expect("tasks");
        tasks.iter().find(|task| task.task_id == task_id).cloned()
    }

    fn handle(&self, method: &str, params: &Value) -> Result<Value, String> {
        let mut tasks = self.tasks.lock().expect("tasks");
        match method {
            "listTasks" => Ok(Value::Array(
                tasks
                    .iter()
                    .filter(|task| !task.archived && !task.pinned)
                    .map(FakeTask::meta)
                    .collect(),
            )),
            "listPinnedTasks" => Ok(Value::Array(
                tasks
                    .iter()
                    .filter(|task| !task.archived && task.pinned)
                    .map(FakeTask::meta)
                    .collect(),
            )),
            "listArchivedTasks" => Ok(Value::Array(
                tasks
                    .iter()
                    .filter(|task| task.archived)
                    .map(FakeTask::meta)
                    .collect(),
            )),
            "listTaskList" => {
                *self.last_search_scopes.lock().expect("scopes") =
                    params.get("workspaceScopes").cloned();
                if *self.fail_search.lock().expect("fail flag") {
                    return Err("search backend unavailable".to_owned());
                }
                let search = params["search"].as_str().unwrap_or_default().to_lowercase();
                let items: Vec<Value> = tasks
                    .iter()
                    .filter(|task| !task.archived)
                    .filter(|task| task.title.to_lowercase().contains(&search))
                    .map(FakeTask::meta)
                    .collect();
                Ok(json!({ "items": items, "total": items.len(), "hasMore": false }))
            }
            "setTaskPinned" | "archiveTask" | "unarchiveTask" | "deleteTask" | "setTaskUnread" => {
                let task_id = params["taskId"].as_str().unwrap_or_default();
                let Some(task) = tasks.iter_mut().find(|task| task.task_id == task_id) else {
                    return Err(format!("task not found: {task_id}"));
                };
                match method {
                    "setTaskPinned" => task.pinned = params["pinned"] == Value::Bool(true),
                    "archiveTask" => task.archived = true,
                    "unarchiveTask" => task.archived = false,
                    "setTaskUnread" => task.unread = params["unread"] == Value::Bool(true),
                    _ => {}
                }
                task.updated_at += 1;
                let meta = task.meta();
                if method == "deleteTask" {
                    tasks.retain(|task| task.task_id != task_id);
                }
                Ok(meta)
            }
            "deleteArchivedTask" => {
                let task_id = params["taskId"].as_str().unwrap_or_default();
                let before = tasks.len();
                tasks.retain(|task| !(task.task_id == task_id && task.archived));
                Ok(Value::Bool(tasks.len() != before))
            }
            "deleteArchivedTasks" => {
                let ids: Vec<String> = params["taskIds"]
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut deleted = Vec::new();
                let mut skipped = Vec::new();
                for id in ids {
                    if tasks.iter().any(|task| task.task_id == id && task.archived) {
                        tasks.retain(|task| task.task_id != id);
                        deleted.push(Value::String(id));
                    } else {
                        skipped.push(Value::String(id));
                    }
                }
                Ok(json!({
                    "deletedTaskIds": deleted,
                    "skippedTaskIds": skipped,
                    "failedTaskIds": [],
                }))
            }
            other => Err(format!("unknown command {other}")),
        }
    }
}

/// 帧头 + 实参。
fn decode_request(frame: &[u8]) -> (i32, i32, String, String, Value) {
    // 出站是完整线帧（含 13 字节 Regular 头），先分帧再解 WireValue。
    let mut frames = crate::frame::FrameReader::new();
    frames.push(frame);
    let payload = frames
        .next_frame()
        .expect("frame parses")
        .expect("complete frame");
    let (header, used) = WireValue::decode(&payload).expect("header decodes");
    let (body, _) = WireValue::decode(&payload[used..]).expect("body decodes");
    let WireValue::Array(items) = header else {
        panic!("request header must be an array");
    };
    let int = |value: &WireValue| match value {
        WireValue::Int(number) => *number,
        other => panic!("expected int in header: {other:?}"),
    };
    let text = |value: &WireValue| match value {
        WireValue::String(text) => text.clone(),
        other => panic!("expected string in header: {other:?}"),
    };
    let arg = body.to_json().unwrap_or(Value::Null);
    let arg = arg
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(arg);
    (
        int(&items[0]),
        int(&items[1]),
        text(&items[2]),
        text(&items[3]),
        arg,
    )
}

fn spawn_fake_task_server(
    client: Arc<RemoteChannelClient>,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    store: Store,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = outbound.recv().await {
            let (kind, id, channel, method, params) = decode_request(&frame);
            if channel != TASK_CHANNEL || kind != 100 {
                continue;
            }
            let result = store.handle(&method, &params);
            let mut payload = WireValue::Array(vec![
                WireValue::Int(RESPONSE_PROMISE_SUCCESS),
                WireValue::Int(id),
            ])
            .encode();
            match result {
                Ok(value) => WireValue::Object(value).encode_into(&mut payload),
                Err(message) => {
                    let mut error = WireValue::Array(vec![
                        WireValue::Int(RESPONSE_PROMISE_ERROR),
                        WireValue::Int(id),
                    ])
                    .encode();
                    WireValue::Object(json!({ "name": "Error", "message": message }))
                        .encode_into(&mut error);
                    payload = error;
                }
            }
            client.handle_frame(&payload).expect("response frame");
        }
    })
}

/// 模拟远端未注册 `escode-task`：所有调用都回显式错误（真实 ChannelServer 超时后同样回错误）。
fn spawn_unknown_channel_server(
    client: Arc<RemoteChannelClient>,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = outbound.recv().await {
            let (kind, id, _channel, _method, _params) = decode_request(&frame);
            if kind != 100 {
                continue;
            }
            let mut payload = WireValue::Array(vec![
                WireValue::Int(RESPONSE_PROMISE_ERROR),
                WireValue::Int(id),
            ])
            .encode();
            WireValue::Object(json!({
                "name": "Unknown channel",
                "message": "Channel name 'escode-task' timed out",
            }))
            .encode_into(&mut payload);
            client.handle_frame(&payload).expect("error frame");
        }
    })
}

fn address(identity: &str, source_server_id: &str, task_id: &str) -> Value {
    json!({
        "taskId": task_id,
        "workspacePath": "/remote/workspace",
        "workspaceIdentity": identity,
        "sourceServerId": source_server_id,
    })
}

async fn controller_with(
    identity: &str,
    tasks: Vec<FakeTask>,
) -> (
    Arc<super::RemoteController>,
    Store,
    tokio::task::JoinHandle<()>,
) {
    let (client, outbound) = initialized_client(&[TASK_CHANNEL]);
    let store = Store::with(tasks);
    let server = spawn_fake_task_server(Arc::clone(&client), outbound, store.clone());
    let controller = RemoteController::new(client, &scope(identity));
    (controller, store, server)
}

/// 验收 1：远端列表来自 `escode-task`，行地址带 `sourceServerId` + stable identity。
#[tokio::test]
async fn lists_remote_tasks_and_exposes_source_identity_on_rows() {
    let (controller, _store, server) = controller_with(
        IDENTITY,
        vec![
            FakeTask::new("task-1", "Alpha", 10),
            FakeTask {
                pinned: true,
                ..FakeTask::new("task-2", "Pinned", 20)
            },
        ],
    )
    .await;

    let result = controller
        .list_task_list(json!({"kind": "active", "sortBy": "updated"}))
        .await
        .expect("remote list");
    let items = result["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["taskId"], json!("task-2"), "updatedAt 降序");
    assert_eq!(items[0]["sourceServerId"], json!(SOURCE));
    assert_eq!(items[0]["workspaceIdentity"], json!(IDENTITY));

    let rows = controller.projector().rows(&[]);
    let row = rows
        .iter()
        .find(|row| row["address"]["taskId"] == json!("task-1"))
        .expect("task row");
    assert_eq!(row["address"]["sourceServerId"], json!(SOURCE));
    assert_eq!(row["address"]["workspaceIdentity"], json!(IDENTITY));
    assert_eq!(row["address"]["workspacePath"], json!("/remote/workspace"));
    assert!(row["address"].get("remoteSessionId").is_none());
    server.abort();
}

/// 本地 scope 的投影键与地址保持不变；远端键包含 `sourceServerId`。
#[test]
fn remote_scope_keys_are_isolated_from_local() {
    let local = LocalScope::new("/local/workspace", Some("local-workspace"));
    assert_eq!(local.source_key(), "local\0local-workspace");
    assert!(local.address("task-1").get("sourceServerId").is_none());

    let remote = LocalScope::remote("/remote/workspace", IDENTITY, SOURCE);
    assert_eq!(remote.source_key(), format!("source\0{SOURCE}\0{IDENTITY}"));
    assert_eq!(remote.address("task-1")["sourceServerId"], json!(SOURCE));
}

/// 搜索分支：Host 把查询收敛到本连接 scope，并回带远端任务与 `sourceServerId`。
#[tokio::test]
async fn search_converges_to_the_bound_scope_and_decorates_items() {
    let (controller, store, server) =
        controller_with(IDENTITY, vec![FakeTask::new("task-1", "Alpha", 10)]).await;
    controller
        .list_task_list(json!({"kind": "active"}))
        .await
        .expect("initial refresh");

    let result = controller
        .list_task_list(json!({
            "kind": "active",
            "sortBy": "updated",
            "search": "alp",
            "workspaceScopes": [{
                "workspacePath": "/remote/other",
                "workspaceIdentity": "remote:server:55555555-5555-4555-8555-555555555555:/remote/other",
            }],
        }))
        .await
        .expect("remote search");
    let items = result["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["taskId"], json!("task-1"));
    assert_eq!(items[0]["sourceServerId"], json!(SOURCE));
    assert_eq!(items[0]["sourceAvailability"], json!("online"));
    assert_eq!(items[0]["liveStatus"], json!("completed"));
    assert_eq!(
        *store.last_search_scopes.lock().expect("scopes"),
        Some(json!([{
            "workspacePath": "/remote/workspace",
            "workspaceIdentity": IDENTITY,
        }]))
    );
    server.abort();
}

#[path = "remote_tests_unread.rs"]
mod unread_tests;
#[cfg(test)]
#[path = "remote_tests_write.rs"]
mod write;
