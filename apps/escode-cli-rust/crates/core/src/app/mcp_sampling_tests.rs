//! 采样账本与作业终态的验收单测（docs/specs/rust-mcp-client-events.md §4）。
use super::*;

fn scope(token: &str, session: &str) -> Scope {
    Scope {
        token: token.into(),
        session: session.into(),
    }
}

#[test]
fn sampling_ledger_rejects_duplicates_and_busy_instances() {
    let mut calls = Calls::default();
    let scope = scope("token", "session");
    let cancel = calls.begin(&scope, "op-1").expect("first");
    assert!(
        calls.begin(&scope, "op-1").is_err(),
        "重复 operationId 必须拒绝"
    );
    assert!(
        calls.begin(&scope, "op-2").is_err(),
        "同实例同时只允许 1 个操作"
    );
    calls.finish(&scope, "op-1");
    assert!(!cancel.is_cancelled());
    assert!(
        calls.begin(&scope, "op-1").is_err(),
        "终态 operationId 不可重放"
    );
    assert!(calls.begin(&scope, "op-2").is_ok());
}

#[test]
fn cancel_before_sample_leaves_a_terminal_entry() {
    let mut calls = Calls::default();
    let scope = scope("token", "session");
    assert!(!calls.cancel(&scope, "op-1"));
    assert!(
        calls.begin(&scope, "op-1").is_err(),
        "取消先到时旧 operationId 不执行"
    );
    assert!(calls.begin(&scope, "op-2").is_ok());
    assert!(calls.cancel(&scope, "op-2"));
    assert!(!calls.cancel(&scope, "op-2"), "取消幂等");
}

#[test]
fn cancel_an_active_operation_cancels_its_token() {
    let mut calls = Calls::default();
    let scope = scope("token", "session");
    let token = calls.begin(&scope, "op").expect("begin");
    assert!(calls.cancel(&scope, "op"));
    assert!(token.is_cancelled(), "在飞采样必须收到取消");
    calls.finish(&scope, "op");
}

#[test]
fn session_limit_is_enforced() {
    let mut calls = Calls::default();
    for index in 0..MAX_PER_SESSION {
        let scoped = scope(&format!("token-{index}"), "session");
        calls.begin(&scoped, "op").expect("within limit");
    }
    assert!(
        calls.begin(&scope("token-extra", "session"), "op").is_err(),
        "同一会话超过并发上限必须拒绝"
    );
    let other = scope("token-other", "other-session");
    assert!(calls.begin(&other, "op").is_ok());
}

mod outcome {
    use super::*;

    #[tokio::test]
    async fn sampling_timeout_reports_32000() {
        let result = sampling_outcome(
            CancellationToken::new(),
            Duration::from_millis(10),
            std::future::pending::<std::result::Result<Value, String>>(),
        )
        .await;
        assert_eq!(
            result.expect_err("timeout"),
            (-32000, "Sampling timed out".to_owned())
        );
    }

    #[tokio::test]
    async fn instance_invalidation_reports_32000() {
        let instance = CancellationToken::new();
        instance.cancel();
        let result = sampling_outcome(
            instance,
            Duration::from_secs(1),
            std::future::pending::<std::result::Result<Value, String>>(),
        )
        .await;
        let (code, message) = result.expect_err("instance invalid");
        assert_eq!(code, -32000);
        assert!(message.contains("no longer authorized"));
    }

    #[test]
    fn ledger_error_codes_match_the_spec() {
        let mut calls = Calls::default();
        let scope = Scope {
            token: "token".into(),
            session: "session".into(),
        };
        calls.begin(&scope, "op-1").expect("first");
        let busy = calls.begin(&scope, "op-2").expect_err("busy");
        assert_eq!(
            busy.downcast_ref::<RpcFailure>()
                .map(|failure| failure.code),
            Some(-32000)
        );
        let duplicate = calls.begin(&scope, "op-1").expect_err("duplicate");
        assert_eq!(
            duplicate
                .downcast_ref::<RpcFailure>()
                .map(|failure| failure.code),
            Some(-32602)
        );
    }
}
