//! 基线 meta 与终态未读裁决的投影（TS `buildBaselineMetaFromSummary` /
//! `resolveTerminalUnreadSignal`）。
use std::sync::Arc;

use crate::agent_v4::base::Base;
use crate::domains_task_support::{LazyIndex, ReadContext};
use crate::task_index_syncer::Ingest;
use crate::task_targets::TaskTargets;
use crate::workspace_events::WorkspaceEvents;
use serde_json::json;

fn summary(id: &str, phase: &str, title: &str) -> serde_json::Value {
    json!({
        "sessionId": id,
        "workspaceId": "/w",
        "title": title,
        "phase": phase,
        "sessionEnded": false,
        "hasBackgroundWork": false,
        "lastActivityAt": 100,
        "createdAt": 90,
    })
}

#[test]
fn baseline_meta_and_unread_signal_follow_the_ts_projections() {
    let dir = tempfile::tempdir().unwrap();
    let ingest = Arc::new(Ingest::new(
        Arc::new(Base::new(
            escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded),
            Arc::new(|_| unreachable!()),
            None,
        )),
        ReadContext {
            path: dir.path().join("tasks-index.sqlite"),
            index: Arc::new(std::sync::Mutex::new(LazyIndex::default())),
            targets: Arc::new(TaskTargets::new()),
            runtime: None,
        },
        Arc::new(WorkspaceEvents::default()),
        "c".to_owned(),
        "/w".to_owned(),
        Some("ssh:host".to_owned()),
        "ssh:host".to_owned(),
    ));
    let mut summary_value = summary("s9", "running", "标题");
    summary_value["parentSessionId"] = json!("p1");
    summary_value["titleSource"] = json!("custom");
    let meta = crate::task_index_syncer::apply_writes::baseline_meta(&ingest, &summary_value);
    assert_eq!(meta["taskId"], "s9");
    assert_eq!(meta["workspaceIdentity"], "ssh:host");
    assert_eq!(meta["forkedFromTaskId"], "p1");
    assert_eq!(meta["titleOverridden"], true);
    assert_eq!(meta["status"], "running");
    assert_eq!(meta["mode"], "default");
    assert_eq!(meta["provider"], "glm");
    assert_eq!(
        meta["traceId"].as_str().map(str::len),
        Some(36),
        "traceId is a fresh uuid v4"
    );
    // phase → status 映射与 TS taskStatusFromSummaryPhase 一致。
    for (phase, status) in [
        ("prewarming", "running"),
        ("completedSuccess", "completed"),
        ("completedInterrupted", "completed"),
        ("error", "error"),
    ] {
        summary_value["phase"] = json!(phase);
        let meta = crate::task_index_syncer::apply_writes::baseline_meta(&ingest, &summary_value);
        assert_eq!(meta["status"], status, "{phase}");
    }
    // 终态未读裁决：error 恒提醒；成功要求无 goal 或已验证。
    summary_value["phase"] = json!("error");
    assert_eq!(
        crate::task_index_syncer::apply::resolve_terminal_unread_signal(&summary_value),
        Some("background_terminal")
    );
    summary_value["phase"] = json!("completedSuccess");
    assert_eq!(
        crate::task_index_syncer::apply::resolve_terminal_unread_signal(&summary_value),
        Some("background_terminal")
    );
    summary_value["goalStatus"] = json!("pending");
    assert_eq!(
        crate::task_index_syncer::apply::resolve_terminal_unread_signal(&summary_value),
        None
    );
    summary_value["goalStatus"] = json!("verified");
    assert_eq!(
        crate::task_index_syncer::apply::resolve_terminal_unread_signal(&summary_value),
        Some("background_terminal")
    );
}
