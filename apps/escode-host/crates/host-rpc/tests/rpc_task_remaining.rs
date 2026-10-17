//! S9–S12 端到端：initialize / goalSession / Claude 导入 / Bot group 7 方法。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn remaining_task_members_reach_the_runtime_and_host_state() {
    let mut fixture = fixture().await;
    let task_id = fixture.task_id.clone();
    let workspace = fixture.workspace.clone();

    // initialize：实例已由 createTask 预热，协议常量固定。
    let initialized =
        object(call_task(&mut fixture, 10, "initialize", serde_json::json!({})).await);
    assert_eq!(initialized["available"], true);
    assert_eq!(initialized["version"], "ESCode Protocol/1");

    // goal show / clear / set。
    let show = object(
        call_task(
            &mut fixture,
            11,
            "goalSession",
            serde_json::json!({"action": "show"}),
        )
        .await,
    );
    assert_eq!(
        show["response"],
        "No goal is set. Use /goal <objective> to set one."
    );
    assert_eq!(show["snapshot"]["session"]["sessionId"], task_id);
    let clear = object(
        call_task(
            &mut fixture,
            12,
            "goalSession",
            serde_json::json!({"action": "clear"}),
        )
        .await,
    );
    assert_eq!(clear["response"], "No goal to clear.");
    let set = object(
        call_task(
            &mut fixture,
            13,
            "goalSession",
            serde_json::json!({
                "action": "set",
                "objective": "finish the migration",
                "inputId": "goal-input-1",
            }),
        )
        .await,
    );
    assert!(
        set["response"]
            .as_str()
            .unwrap()
            .starts_with("Goal active\nObjective: finish the migration"),
        "{set}"
    );
    assert_eq!(set["snapshot"]["session"]["sessionId"], task_id);
    // pause 可打断运行中的续跑；之后 clear 经 V4 clearGoal 删除目标。
    let paused = object(
        call_task(
            &mut fixture,
            499,
            "goalSession",
            serde_json::json!({"action": "pause"}),
        )
        .await,
    );
    assert_eq!(paused["response"], "", "{paused}");
    let mut cleared = serde_json::Value::Null;
    for attempt in 0..50 {
        cleared = object(
            call_task(
                &mut fixture,
                500 + attempt,
                "goalSession",
                serde_json::json!({"action": "clear"}),
            )
            .await,
        );
        // 被暂停的续跑收尾前 clear 会以 activeTurn 拒绝；收尾后才能删除。
        if cleared.get("response").is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(cleared["response"], "Goal cleared.", "{cleared}");
    assert!(cleared["snapshot"]["session"]["target"].is_null());

    // Claude 扫描：dataBaseDir 下的假 HOME 也会被扫描。
    let candidate_path = fixture
        .dir
        .path()
        .join("data-base")
        .join(".claude")
        .join("projects")
        .join("-repo")
        .join("claude-s1.jsonl");
    std::fs::create_dir_all(candidate_path.parent().unwrap()).unwrap();
    let records = [
        serde_json::json!({"type":"user","cwd":workspace,"timestamp":1_700_000_000_000_i64,
            "message":{"content":"imported question"}}),
        serde_json::json!({"type":"assistant","timestamp":1_700_000_001_000_i64,
            "message":{"model":"claude-x","content":"imported answer"}}),
    ];
    std::fs::write(
        &candidate_path,
        records
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    call(
        &mut fixture.socket,
        14,
        "escode-task",
        "scanImportableClaudeSessions",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
        }))]),
    )
    .await;
    let Wire::Array(candidates) = read_response(&mut fixture.socket, 14).await else {
        panic!("scan 应答应为数组")
    };
    assert!(
        candidates
            .iter()
            .any(|candidate| matches!(candidate, Wire::Object(value)
            if value["sessionId"] == "claude-s1")),
        "{candidates:?}"
    );

    call(
        &mut fixture.socket,
        15,
        "escode-task",
        "importClaudeSessions",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "sessionIds": ["claude-s1"],
        }))]),
    )
    .await;
    let imported = object(read_response(&mut fixture.socket, 15).await);
    assert_eq!(
        imported["imported"][0]["sessionId"], "claude-s1",
        "{imported}"
    );
    assert_eq!(
        imported["failed"].as_array().unwrap().len(),
        0,
        "{imported}"
    );
    let imported_task = imported["imported"][0]["taskId"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &mut fixture.socket,
        16,
        "escode-task",
        "getTaskMeta",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "workspacePath": workspace,
            "taskId": imported_task,
        }))]),
    )
    .await;
    let meta = object(read_response(&mut fixture.socket, 16).await);
    assert_eq!(meta["migrationSource"], "claudeCode", "{meta}");
    // 导入建出的是 runtime 会话（不再是只进列表的 legacy 快照）：可 resume，快照带导入正文。
    call(
        &mut fixture.socket,
        600,
        "escode-task",
        "resumeTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": imported_task,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
            "model": {"providerId": "fixture", "modelId": "fixture-model",
                "options": {"reasoningLevel": "none"}},
            "thoughtLevel": "none",
        }))]),
    )
    .await;
    let resumed = object(read_response(&mut fixture.socket, 600).await);
    assert_eq!(resumed["taskId"], imported_task.as_str(), "{resumed}");
    call(
        &mut fixture.socket,
        601,
        "escode-task",
        "getTaskSnapshot",
        &Wire::Array(vec![Wire::Object(serde_json::json!({
            "taskId": imported_task,
            "workspacePath": workspace,
            "workspaceIdentity": workspace,
        }))]),
    )
    .await;
    let snapshot = object(read_response(&mut fixture.socket, 601).await);
    let text = snapshot.to_string();
    assert!(
        text.contains("imported question") && text.contains("imported answer"),
        "{snapshot}"
    );

    // Bot group：快照/队列/build 状态。
    let summaries = call_task(
        &mut fixture,
        17,
        "readBotTopicSummaries",
        serde_json::json!({}),
    )
    .await;
    assert!(matches!(summaries, Wire::Array(_)), "{summaries:?}");
    let invalidated = call_task(
        &mut fixture,
        18,
        "invalidateBotTopicInputs",
        serde_json::json!({"remoteSessionId": null}),
    )
    .await;
    assert_eq!(invalidated, Wire::Undefined);
    let block = call_task(
        &mut fixture,
        19,
        "getBotGroupTaskBlockReason",
        serde_json::json!({}),
    )
    .await;
    assert!(
        matches!(&block, Wire::Object(serde_json::Value::Null) | Wire::Str(_)),
        "{block:?}"
    );
    let execution = call_task(
        &mut fixture,
        20,
        "readBotTopicExecution",
        serde_json::json!({}),
    )
    .await;
    assert!(
        matches!(&execution, Wire::Undefined | Wire::Object(_)),
        "{execution:?}"
    );
    let cancel = object(
        call_task(
            &mut fixture,
            21,
            "cancelBotGroupInput",
            serde_json::json!({
                "sourceCommandId": "missing",
                "commandId": "cancel-1",
                "botId": "bot-1",
                "chatId": "chat-1",
                "ownerId": "owner-1",
                "actorId": "owner-1",
            }),
        )
        .await,
    );
    assert_eq!(cancel["message"], "Input is no longer queued");
    let stop = call_task(
        &mut fixture,
        22,
        "stopBotTopicExecution",
        serde_json::json!({"executionId": "missing-execution"}),
    )
    .await;
    assert_eq!(stop, Wire::Undefined);

    // 提交无附件输入：ack 应被接受（模型端点不可达属于后续轮次失败）。
    let submitted = object(
        call_task(
            &mut fixture,
            23,
            "submitBotGroupInput",
            serde_json::json!({
                "commandId": "bot-command-1",
                "content": "bot hello",
                "source": {
                    "provider": "feishu",
                    "botId": "bot-1",
                    "chatId": "chat-1",
                    "senderId": "user-1",
                    "senderName": "User",
                    "messageId": "msg-1",
                },
                "attachments": [],
            }),
        )
        .await,
    );
    assert_eq!(submitted["status"], "accepted", "{submitted}");

    call(
        &mut fixture.socket,
        24,
        "escode-task",
        "closeTask",
        &Wire::Array(vec![Wire::Object(serde_json::json!({"taskId": task_id}))]),
    )
    .await;
    assert_eq!(
        read_response(&mut fixture.socket, 24).await,
        Wire::Undefined
    );
    fixture.surface.shutdown().await;
}
