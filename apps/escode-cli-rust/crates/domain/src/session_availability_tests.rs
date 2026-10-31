use super::*;
use serde_json::json;

fn session() -> Session {
    Session::new(
        "sess".into(),
        "workspace".into(),
        "provider".into(),
        "model".into(),
        "high".into(),
        "epoch".into(),
        1,
    )
}

fn goal(status: &str) -> crate::goal::Goal {
    serde_json::from_value(json!({
        "targetId":"target","objective":"objective","status":status,"iteration":0,
        "verifications":[],"iterations":[],"tokensUsed":0,"tokenBudget":null,
        "timeUsedMs":0,"activeRunStartedAtMs":null,"lastSeen":null
    }))
    .expect("goal fixture")
}

fn running() -> Session {
    let mut s = session();
    s.phase = "running".into();
    s.run_id = Some("run".into());
    s
}

#[test]
fn draft_denies_compact_fork_and_send_now_with_product_reason_codes() {
    let s = session();
    let availability = s.availability();
    assert_eq!(
        availability["compact"],
        json!({"allowed":false,"reasonCode":"idleCannotCompact"})
    );
    assert_eq!(
        availability["fork"],
        json!({"allowed":false,"reasonCode":"forkTargetNotStable"})
    );
    assert_eq!(
        availability["sendQueuedNow"],
        json!({"allowed":false,"reasonCode":"sendQueuedNowRequiresRunning"})
    );
}

#[test]
fn running_allows_compact_fork_and_send_now() {
    let availability = running().availability();
    assert_eq!(availability["compact"], json!({"allowed":true}));
    assert_eq!(availability["fork"], json!({"allowed":true}));
    assert_eq!(availability["sendQueuedNow"], json!({"allowed":true}));
}

#[test]
fn compacting_locks_compact_fork_and_send_now() {
    let mut s = running();
    s.compact_instructions = Some(String::new());
    let availability = s.availability();
    for key in ["compact", "fork", "sendQueuedNow"] {
        assert_eq!(
            availability[key],
            json!({"allowed":false,"reasonCode":"compactOperationLock"}),
            "{key}"
        );
    }
}

#[test]
fn completed_session_allows_compact_and_fork_but_not_send_now() {
    let mut s = session();
    s.phase = "completedSuccess".into();
    let availability = s.availability();
    assert_eq!(availability["compact"], json!({"allowed":true}));
    assert_eq!(availability["fork"], json!({"allowed":true}));
    assert_eq!(
        availability["sendQueuedNow"],
        json!({"allowed":false,"reasonCode":"sendQueuedNowRequiresRunning"})
    );
}

#[test]
fn goal_actions_follow_node_reason_codes() {
    let mut s = session();
    assert_eq!(
        s.availability()["pauseGoal"],
        json!({"allowed":false,"reasonCode":"noGoalToPause"})
    );
    assert_eq!(
        s.availability()["resumeGoal"],
        json!({"allowed":false,"reasonCode":"noGoalToResume"})
    );
    for status in ["active", "verifying", "notSatisfied"] {
        s.goal = Some(goal(status));
        assert_eq!(s.availability()["pauseGoal"], json!({"allowed":true}));
        assert_eq!(
            s.availability()["resumeGoal"],
            json!({"allowed":false,"reasonCode":"goalNotPaused"})
        );
    }
    s.goal = Some(goal("paused"));
    assert_eq!(s.availability()["resumeGoal"], json!({"allowed":true}));
    assert_eq!(
        s.availability()["pauseGoal"],
        json!({"allowed":false,"reasonCode":"goalNotActive"})
    );
    // Node 只允许 paused 恢复：failed 也必须报 goalNotPaused。
    s.goal = Some(goal("failed"));
    assert_eq!(
        s.availability()["resumeGoal"],
        json!({"allowed":false,"reasonCode":"goalNotPaused"})
    );
}

#[test]
fn patch_and_snapshot_share_the_same_availability() {
    let s = running();
    assert_eq!(s.patch()["availability"], s.availability());
    assert_eq!(s.snapshot()["availability"], s.availability());
}

#[test]
fn snapshot_carries_null_model_transition_but_patches_omit_it() {
    let s = running();
    assert_eq!(s.snapshot()["modelTransition"], Value::Null);
    assert!(s.patch().get("modelTransition").is_none());
    assert_eq!(
        s.snapshot()["subagents"],
        json!({"revision":0,"childSessionIds":[],"running":[],"endedTotal":0})
    );
    assert_eq!(s.snapshot()["workspaceHookAdmission"], Value::Null);
}

#[test]
fn recovery_drops_interrupted_compact_state() {
    let mut s = running();
    s.compact_instructions = Some("instructions".into());
    s.recover("next".into(), 2);
    assert!(s.compact_instructions.is_none());
    assert_eq!(s.phase, "completedInterrupted");
    assert_eq!(s.availability()["compact"], json!({"allowed":true}));
}
