use super::*;

/// 一行最小合法 meta_json。
fn valid_meta() -> Value {
    json!({
        "taskId": "task_1",
        "traceId": "escode-task_1",
        "title": "hello",
        "workspacePath": "/w",
        "createdAt": 100,
        "updatedAt": 200,
        "mode": "build",
    })
}

/// 组装一行（与 `TASK_ROW_SELECT` 的键同名）。
fn row(meta: Value) -> Value {
    json!({
        "workspace_key": "/w",
        "workspace_path": "/w",
        "workspace_identity": Value::Null,
        "task_id": "task_1",
        "title": "列标题",
        "task_status": "running",
        "provider": "glm",
        "mode": "build",
        "model": "glm-4",
        "migration_source": Value::Null,
        "forked_from_task_id": Value::Null,
        "cron_automation_id": Value::Null,
        "created_at": 100,
        "updated_at": 200,
        "unread_at": Value::Null,
        "title_overridden": 0,
        "meta_json": meta.to_string(),
    })
}

#[test]
fn valid_meta_json_takes_the_projection_path() {
    let meta = to_task_meta(&row(valid_meta())).expect("meta");
    assert_eq!(meta["taskId"], "task_1");
    assert_eq!(meta["title"], "hello", "meta 里的标题优先于列");
    assert_eq!(meta["titleOverridden"], false);
    assert!(
        meta.get("unreadAt").is_none(),
        "unread_at 为空时不投射该键（TS undefined 语义）"
    );
}

#[test]
fn scalar_columns_override_meta_and_fill_cron_identity() {
    let mut meta = valid_meta();
    meta["cronAutomationId"] = json!("cron_1");
    meta["unreadAt"] = json!(999);
    let mut candidate = row(meta);
    // 库里 cron 列与 meta 不一致：meta 为准；unread 以列覆盖 meta。
    candidate["cron_automation_id"] = json!("cron_column");
    candidate["unread_at"] = json!(7);
    let projected = to_task_meta(&candidate).expect("meta");
    assert_eq!(projected["cronAutomationId"], "cron_1");
    assert_eq!(projected["unreadAt"], 7);
}

#[test]
fn cron_falls_back_to_the_column_when_meta_lacks_it() {
    let mut candidate = row(valid_meta());
    candidate["cron_automation_id"] = json!("cron_column");
    let projected = to_task_meta(&candidate).expect("meta");
    assert_eq!(projected["cronAutomationId"], "cron_column");
}

#[test]
fn invalid_meta_json_falls_back_to_the_column_projection() {
    // meta_json 不是 JSON。
    let mut broken_row = row(valid_meta());
    broken_row["meta_json"] = json!("{not json");
    let projected = to_task_meta(&broken_row).expect("fallback");
    assert_eq!(projected["title"], "列标题", "回退用列标题");
    assert_eq!(projected["traceId"], "escode-task_1");
    assert_eq!(projected["mode"], "build");
    assert_eq!(projected["model"], "glm-4");
    assert_eq!(projected["provider"], "glm");
    assert_eq!(projected["status"], "running");

    // 必填缺失 / 枚举非法都走回退。
    for meta in [
        json!({"traceId": "t", "title": "x", "workspacePath": "/w", "createdAt": 1, "updatedAt": 2, "mode": "build"}),
        json!({"taskId": "task_1", "traceId": "t", "title": "x", "workspacePath": "/w", "createdAt": 1, "updatedAt": 2, "mode": "not-a-mode"}),
        json!({"taskId": "task_1", "traceId": "t", "title": "x", "workspacePath": "/w", "createdAt": -1, "updatedAt": 2, "mode": "build"}),
    ] {
        let mut candidate = row(valid_meta());
        candidate["meta_json"] = json!(meta.to_string());
        let projected = to_task_meta(&candidate).expect("fallback");
        assert_eq!(projected["title"], "列标题", "{meta}");
    }
}

#[test]
fn workspace_identity_follows_the_ts_resolution_rules() {
    // 三者相等（workspace_key == path == identity）：没有独立身份。
    let mut candidate = row(valid_meta());
    candidate["workspace_identity"] = json!("/w");
    assert!(
        to_task_meta(&candidate)
            .unwrap()
            .get("workspaceIdentity")
            .is_none()
    );

    // identity 与 workspace_key 一致：保留。
    let mut candidate = row(valid_meta());
    candidate["workspace_key"] = json!("remote:ssh:dev");
    candidate["workspace_identity"] = json!("remote:ssh:dev");
    assert_eq!(
        to_task_meta(&candidate).unwrap()["workspaceIdentity"],
        "remote:ssh:dev"
    );

    // workspace_key 是远端身份而列里是别的东西：以 workspace_key 为准。
    let mut candidate = row(valid_meta());
    candidate["workspace_key"] = json!("remote:ssh:dev");
    candidate["workspace_identity"] = json!("stale");
    assert_eq!(
        to_task_meta(&candidate).unwrap()["workspaceIdentity"],
        "remote:ssh:dev"
    );

    // 本地不一致的残留 identity：不采用（避免投影到另一个远端）。
    let mut candidate = row(valid_meta());
    candidate["workspace_identity"] = json!("stale-local");
    assert!(
        to_task_meta(&candidate)
            .unwrap()
            .get("workspaceIdentity")
            .is_none()
    );
}
