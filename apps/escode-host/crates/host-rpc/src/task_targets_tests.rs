use super::*;

fn target(task_id: &str, cron: Option<&str>) -> TaskTarget {
    TaskTarget {
        task_id: task_id.to_owned(),
        workspace_path: "/w".to_owned(),
        workspace_identity: None,
        cron_automation_id: cron.map(str::to_owned),
        remote_session_id: None,
    }
}

#[test]
fn remember_preserves_existing_cron_ownership() {
    let targets = TaskTargets::new();
    targets.remember(target("task_1", Some("cron_1")));
    // 快照/恢复入口不带 cron 标记：既有归属必须保留（TS `rememberTaskTarget` 的 merge-preserve）。
    targets.remember(target("task_1", None));
    assert_eq!(
        targets.get("task_1").unwrap().cron_automation_id.as_deref(),
        Some("cron_1")
    );
}

#[test]
fn remember_indexed_meta_is_authoritative_and_can_clear_cron() {
    let targets = TaskTargets::new();
    targets.remember(target("task_1", Some("cron_1")));
    // 索引 meta 是权威来源：解绑 automation 后必须真的清空。
    targets.remember_indexed_meta("task_1", "/w", Some("/w-identity"), None);
    let stored = targets.get("task_1").unwrap();
    assert_eq!(stored.cron_automation_id, None);
    assert_eq!(stored.workspace_identity.as_deref(), Some("/w-identity"));
}

#[test]
fn unknown_task_reports_the_typed_error() {
    let targets = TaskTargets::new();
    let error = targets.get("task_missing").expect_err("explicit error");
    assert_eq!(error.0, "ESCODE_SESSION_TARGET_NOT_FOUND");
    assert_eq!(error.1, "ESCode session target is not loaded: task_missing");
}

#[test]
fn remember_from_meta_reads_the_index_row_shape() {
    let targets = TaskTargets::new();
    remember_from_meta(
        &targets,
        &serde_json::json!({
            "taskId": "task_2",
            "workspacePath": "/w2",
            "workspaceIdentity": "remote:ssh:dev",
            "cronAutomationId": "cron_9",
        }),
    );
    let stored = targets.get("task_2").unwrap();
    assert_eq!(stored.workspace_path, "/w2");
    assert_eq!(stored.workspace_identity.as_deref(), Some("remote:ssh:dev"));
    assert_eq!(stored.cron_automation_id.as_deref(), Some("cron_9"));

    // 缺字段的行不注册（列表项一定带 taskId/workspacePath）。
    remember_from_meta(&targets, &serde_json::json!({"workspacePath": "/w3"}));
    assert!(targets.get("task_3").is_err());
}
