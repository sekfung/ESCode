use super::*;

fn token(value: &str) -> impl FnOnce() -> String {
    let value = value.to_owned();
    move || value
}

#[test]
fn a_reservation_blocks_others_until_it_expires_or_is_released() {
    let b = Broadcast::default();
    let first = b.acquire_at(" k ", 1_000, token("t1"));
    assert_eq!(
        first["lease"],
        json!({"key": "k", "token": "t1"}),
        "键被裁剪"
    );
    let busy = b.acquire_at("k", 1_100, token("t2"));
    assert_eq!(
        busy,
        json!({"status": "busy", "retryAfterMs": 250}),
        "剩余时间超过 250 → 封顶 250"
    );
    let late = b.acquire_at("k", 1_000 + RESERVATION_TTL_MS - 40, token("t2"));
    assert_eq!(late["retryAfterMs"], 40, "快到期时 retry 更短");
    let after = b.acquire_at("k", 1_000 + RESERVATION_TTL_MS, token("t3"));
    assert_eq!(after["lease"]["token"], "t3", "过期后可重新获取");
    b.release_at("k", "wrong", 7_000);
    assert_eq!(
        b.acquire_at("k", 7_001, token("t4"))["status"],
        "busy",
        "迟到/错误 token 不影响现 winner"
    );
    b.release_at("k", "t3", 7_002);
    assert_eq!(b.acquire_at("k", 7_003, token("t5"))["status"], "acquired");
}

#[test]
fn commit_is_permanent_and_only_the_token_holder_can_commit() {
    let b = Broadcast::default();
    b.acquire_at("k", 0, token("t1"));
    b.commit_at("k", "other", 1);
    assert_eq!(b.acquire_at("k", 2, token("x"))["status"], "busy");
    b.commit_at("k", "t1", 3);
    assert_eq!(
        b.acquire_at("k", 10_000_000, token("x")),
        json!({"status": "committed"})
    );
    b.release_at("k", "t1", 4);
    assert_eq!(
        b.acquire_at("k", 5, token("x"))["status"],
        "committed",
        "已提交的不能释放"
    );
    assert_eq!(
        b.acquire_at("   ", 0, token("x")),
        json!({"status": "unavailable"})
    );
}

#[test]
fn the_claim_table_evicts_the_oldest_beyond_its_cap() {
    let b = Broadcast::default();
    for index in 0..=MAX_LOCAL_CLAIMS {
        b.acquire_at(&format!("k{index}"), 0, token("t"));
    }
    let claims = b.claims();
    assert_eq!(claims.entries.len(), MAX_LOCAL_CLAIMS);
    assert!(claims.get("k0").is_none(), "最早插入的被淘汰");
    assert!(claims.get(&format!("k{MAX_LOCAL_CLAIMS}")).is_some());
}

#[test]
fn send_validates_and_fans_out() {
    let b = Broadcast::default();
    let mut one = b.subscribe();
    let mut two = b.subscribe();
    b.send(&json!({"channel": "  state:theme ", "payload": {"a": 1}, "sourceWindowId": 3}))
        .expect("send");
    for receiver in [&mut one, &mut two] {
        assert_eq!(
            receiver.try_recv().expect("message"),
            json!({"channel": "state:theme", "payload": {"a": 1}, "sourceWindowId": 3})
        );
    }
    assert!(b.send(&json!({"channel": "  ", "payload": 1})).is_err());
    assert!(b.send(&json!({"channel": "c"})).is_err(), "缺 payload 键");
    assert!(b.send(&json!({"channel": "c", "payload": null})).is_ok());
    assert!(
        b.send(&json!({"channel": "c", "payload": 1, "sourceWindowId": 1.5}))
            .is_err()
    );
    assert!(
        Broadcast::default()
            .send(&json!({"channel": "c", "payload": 0}))
            .is_ok(),
        "无订阅者不是错误"
    );
}
