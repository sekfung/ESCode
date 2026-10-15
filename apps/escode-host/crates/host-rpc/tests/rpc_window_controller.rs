//! `window-controller` 端到端（docs/specs/rust-u2-window-controller.md）：真 WebSocket + 真索引。
//! 覆盖：订阅 → 首帧快照 → 列表查询触发投影 → 增量帧 → resync/unsubscribe → mutation 与写后面。

mod support;

use serde_json::{Value, json};
use support::*;

fn object(wire: Wire) -> Value {
    wire.into_object().expect("object payload")
}

#[tokio::test(flavor = "multi_thread")]
async fn controller_projects_the_task_list_and_publishes_deltas() {
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
    let mut client = ControllerClient::new_client(&surface).await;
    // 先挂事件监听再订阅（Renderer `windowControllerTaskListRegistry` 的顺序）。
    client.listen_id = client
        .send(
            session::REQUEST_EVENT_LISTEN,
            "window-controller",
            "onDynamicControllerFrame",
            Wire::Undefined,
        )
        .await;

    // 订阅 tasks topic：ACK 与首帧快照（订阅本身不刷新，快照为空投影）。
    let ack = object(
        client
            .call(
                "window-controller",
                "subscribeControllerV4",
                json!({"topic": "controller/tasks-index", "visibility": "foreground"}),
            )
            .await
            .expect("subscribe"),
    );
    assert_eq!(ack["ack"]["mode"], "snapshot");
    let subscription_id = ack["ack"]["subscriptionId"].as_str().unwrap().to_owned();
    let log_epoch = ack["ack"]["logEpoch"].as_str().unwrap().to_owned();
    let first = client
        .controller_frame(|frame| frame["topic"] == "controller/tasks-index")
        .await;
    assert_eq!(
        first["subscriptionId"],
        Value::from(subscription_id.clone())
    );
    assert_eq!(first["logEpoch"], Value::from(log_epoch.clone()));
    assert_eq!(first["fromSeq"], 0);
    assert_eq!(first["payload"]["kind"], "snapshot");
    assert_eq!(
        first["payload"]["snapshot"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // 真会话 → 索引行。
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
    let address = json!({
        "workspacePath": workspace,
        "workspaceIdentity": workspace,
        "taskId": task_id,
    });

    // 列表查询（非搜索分支）：投影首次刷新 → 发布增量帧。
    let listed = object(
        client
            .call(
                "window-controller",
                "listTaskList",
                json!({"kind": "active", "sortBy": "updated",
                    "workspaceScopes": [{"workspacePath": workspace, "workspaceIdentity": workspace}]}),
            )
            .await
            .expect("listTaskList"),
    );
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{listed}");
    assert_eq!(items[0]["taskId"], Value::from(task_id.clone()));
    assert_eq!(items[0]["sourceAvailability"], "online");
    // 列表项 = meta + 叠加字段；membership 只存在于投影行（帧载荷）里。
    assert!(items[0].get("membership").is_none(), "{listed}");
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["hasMore"], false);
    assert!(items[0].get("remoteSessionId").is_none());

    // 搜索分支：先改标题（走 escode-task 写面 → 索引写 + 事件 → controller 刷新），再按标题搜索。
    let title = format!("controller search {task_id}");
    client
        .call(
            "escode-task",
            "renameTask",
            json!({"workspacePath": workspace, "workspaceIdentity": workspace,
                "taskId": task_id, "title": title}),
        )
        .await
        .expect("renameTask");
    let searched = object(
        client
            .call(
                "window-controller",
                "listTaskList",
                json!({"kind": "active", "sortBy": "updated", "search": title,
                    "workspaceScopes": [{"workspacePath": workspace, "workspaceIdentity": workspace}]}),
            )
            .await
            .expect("search"),
    );
    let found = searched["items"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{searched}");
    assert_eq!(found[0]["taskId"], Value::from(task_id.clone()));
    assert_eq!(found[0]["title"], Value::from(title.clone()));
    // 搜索项同样带投影叠加字段（NV3 的落点）。
    assert_eq!(found[0]["sourceAvailability"], "online");
    assert!(found[0]["liveStatus"].is_string(), "{searched}");
    assert_eq!(searched["total"], 1);

    // 首轮刷新把新行作为增量帧发出来（水位严格递增）。
    let delta = client
        .controller_frame(|frame| frame["payload"]["kind"] == "deltas")
        .await;
    assert_eq!(
        delta["subscriptionId"],
        Value::from(subscription_id.clone())
    );
    assert_eq!(
        delta["fromSeq"], first["toSeq"],
        "delta={delta} first={first}"
    );
    assert_eq!(delta["payload"]["deltas"][0]["op"], "task.upserted");
    assert_eq!(
        delta["payload"]["deltas"][0]["task"]["address"]["taskId"],
        Value::from(task_id.clone())
    );

    // resync：重发快照（fromSeq 0，toSeq = 当前水位），snapshot 含该行。
    let resynced = object(
        client
            .call(
                "window-controller",
                "resyncControllerV4",
                json!({"subscriptionId": subscription_id}),
            )
            .await
            .expect("resync"),
    );
    assert_eq!(
        resynced["ack"]["subscriptionId"],
        Value::from(subscription_id.clone())
    );
    let snapshot_frame = client
        .controller_frame(|frame| frame["payload"]["kind"] == "snapshot" && frame["toSeq"] != 0)
        .await;
    assert_eq!(snapshot_frame["fromSeq"], 0);
    assert_eq!(
        snapshot_frame["payload"]["snapshot"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // mutation：pin → 返回投影 meta；pinned 列表命中；投影订阅收到新行。
    let meta = object(
        client
            .call(
                "window-controller",
                "mutateTask",
                json!({"address": address, "mutation": {"kind": "pin", "pinned": true}}),
            )
            .await
            .expect("mutateTask"),
    );
    assert_eq!(meta["taskId"], Value::from(task_id.clone()));
    assert_eq!(meta["workspacePath"], Value::from(workspace.clone()));
    let pinned = object(
        client
            .call(
                "window-controller",
                "listTaskList",
                json!({"kind": "pinned", "sortBy": "updated",
                    "workspaceScopes": [{"workspacePath": workspace, "workspaceIdentity": workspace}]}),
            )
            .await
            .expect("listTaskList pinned"),
    );
    assert_eq!(pinned["items"].as_array().unwrap().len(), 1);
    assert_eq!(pinned["items"][0]["taskId"], Value::from(task_id.clone()));
    // membership 过滤：置顶后不再属于 timeline 分区（`timeline` = 非置顶且非归档）。
    let timeline = object(
        client
            .call(
                "window-controller",
                "listTaskList",
                json!({"kind": "timeline", "sortBy": "updated",
                    "workspaceScopes": [{"workspacePath": workspace, "workspaceIdentity": workspace}]}),
            )
            .await
            .expect("listTaskList timeline"),
    );
    assert_eq!(timeline["items"].as_array().unwrap().len(), 0, "{timeline}");

    // 未订阅号 resync → TS 同文案错误。
    let error = client
        .call(
            "window-controller",
            "resyncControllerV4",
            json!({"subscriptionId": "missing"}),
        )
        .await
        .expect_err("unknown subscription");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("未找到 Controller subscription"),
        "{error}"
    );

    // unsubscribe 后同一订阅号不再可用（幂等 no-op），resync 报错。
    client
        .call(
            "window-controller",
            "unsubscribeControllerV4",
            json!({"subscriptionId": subscription_id}),
        )
        .await
        .expect("unsubscribe");
    assert!(
        client
            .call(
                "window-controller",
                "unsubscribeControllerV4",
                json!({"subscriptionId": subscription_id}),
            )
            .await
            .is_ok(),
        "unsubscribe is idempotent"
    );
    assert!(
        client
            .call(
                "window-controller",
                "resyncControllerV4",
                json!({"subscriptionId": subscription_id}),
            )
            .await
            .is_err(),
        "subscription is gone"
    );

    // workspaces topic：source 事实（本地在线）。
    let ack = object(
        client
            .call(
                "window-controller",
                "subscribeControllerV4",
                json!({"topic": "controller/workspaces"}),
            )
            .await
            .expect("subscribe workspaces"),
    );
    let workspaces = client
        .controller_frame(|frame| frame["topic"] == "controller/workspaces")
        .await;
    assert_eq!(
        workspaces["subscriptionId"],
        ack["ack"]["subscriptionId"].clone()
    );
    let facts = workspaces["payload"]["snapshot"]["workspaces"]
        .as_array()
        .unwrap();
    assert_eq!(facts.len(), 1, "{workspaces}");
    assert_eq!(facts[0]["workspacePath"], Value::from(workspace.clone()));
    assert_eq!(facts[0]["sourceAvailability"], "online");
    assert_eq!(facts[0]["connectionState"], "online");
}
