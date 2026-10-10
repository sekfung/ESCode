use super::*;

fn user(text: &str) -> Value {
    json!({"role":"user","content":text})
}

#[test]
fn first_reminder_is_full_and_waits_five_real_turns() {
    let first = mode_reminder(&[], true).unwrap();
    assert!(
        first["content"]
            .as_str()
            .unwrap()
            .contains("Plan mode is active.")
    );
    let mut history = vec![first, user("a")];
    for _ in 0..3 {
        assert!(mode_reminder(&history, true).is_none());
        history.push(user("b"));
    }
    // 第 5 个真实用户轮次之后才再次提醒，且第二次是精简版。
    assert!(
        mode_reminder(&history, true).is_none(),
        "only 4 real turns so far"
    );
    history.push(user("c"));
    let second = mode_reminder(&history, true).unwrap();
    assert!(
        second["content"]
            .as_str()
            .unwrap()
            .contains("Plan mode still active")
    );
}

#[test]
fn synthetic_messages_do_not_count_as_real_turns() {
    let mut history = vec![mode_reminder(&[], true).unwrap()];
    for _ in 0..6 {
        history.push(user("<system-reminder>\nx\n</system-reminder>"));
    }
    assert!(mode_reminder(&history, true).is_none());
    assert!(mode_reminder(&[], false).is_none());
}

#[test]
fn plan_file_names_follow_ts_sanitizing() {
    assert_eq!(plan_file_name("sess_1").as_deref(), Some("plan-sess_1.md"));
    assert_eq!(plan_file_name(" a/b:c ").as_deref(), Some("plan-a-b-c.md"));
    assert_eq!(plan_file_name("//").as_deref(), None);
}

#[test]
fn answers_normalize_like_the_ts_broker() {
    assert_eq!(decide(&json!({"optionId":"allowOnce"})), Decision::Approve);
    assert_eq!(
        decide(&json!({"freeText":"  add tests "})),
        Decision::Feedback("add tests".into())
    );
    assert_eq!(decide(&json!({"freeText":"approve"})), Decision::Approve);
    assert_eq!(decide(&json!({"optionId":"deny"})), Decision::Decline);
    assert_eq!(
        decide(&json!({"action":"accept","content":{"answer":"approve"}})),
        Decision::Approve
    );
    assert_eq!(decide(&json!({"action":"decline"})), Decision::Decline);
}

#[test]
fn exit_result_matches_ts_text() {
    assert!(exit_result(" p ").ends_with("## Approved Plan:\np"));
    assert_eq!(
        exit_result(""),
        "User has approved exiting plan mode. You can now proceed."
    );
}
