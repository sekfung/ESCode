use super::*;
use std::ffi::OsString;

fn context(env: Arc<EnvSnapshot>, spawner: &Arc<Spawner>) -> Arc<InstanceContext> {
    Arc::new(InstanceContext {
        env,
        spawner: Arc::downgrade(spawner),
        usage_sink: OnceLock::new(),
        media_cache_root: OnceLock::new(),
    })
}

fn fixture_env(marker: &str, value: &str) -> Arc<EnvSnapshot> {
    Arc::new(EnvSnapshot::from_parts(
        [(OsString::from(marker), OsString::from(value))],
        std::path::PathBuf::from("/fixture"),
        vec!["escode".to_owned()],
        std::path::PathBuf::from("/fixture/escode"),
    ))
}

#[tokio::test]
async fn panics_are_reported_and_reraised() {
    let spawner = Spawner::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    spawner.with_panic_sink(Arc::new(move |name, message| {
        record
            .lock()
            .unwrap()
            .push((name.to_owned(), message.to_owned()));
    }));
    let handle = spawner.spawn("boom", async { panic!("detonation") });
    let error = handle.await.expect_err("task must fail");
    assert!(error.is_panic());
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("boom".to_owned(), "detonation".to_owned())]
    );
    assert!(spawner.running().is_empty(), "guard clears the ledger");
}

#[tokio::test]
async fn running_lists_live_tasks_and_drain_waits() {
    let spawner = Spawner::new();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    spawner.spawn("held", async {
        let _ = gate_rx.await;
    });
    assert_eq!(spawner.running(), vec!["held".to_owned()]);
    let _ = gate_tx.send(());
    assert!(spawner.drain(Duration::from_secs(5)).await.is_empty());
}

#[tokio::test]
async fn drain_returns_residual_names_after_grace() {
    let spawner = Spawner::new();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    spawner.spawn("stuck", async {
        let _ = gate_rx.await;
    });
    tokio::task::yield_now().await;
    let residual = spawner.drain(Duration::from_millis(30)).await;
    assert_eq!(residual, vec!["stuck".to_owned()]);
    let _ = gate_tx.send(());
}

/// 极快任务可能在 register 前完成；收口后账目不能留下 stale 条目。
#[tokio::test(flavor = "multi_thread")]
async fn fast_blocking_tasks_leave_no_stale_ledger_entries() {
    let spawner = Spawner::new();
    let mut handles = Vec::new();
    for _ in 0..256 {
        handles.push(spawner.spawn_blocking("fast-blocking", || ()));
    }
    for handle in handles {
        handle.await.expect("fast task joins");
    }
    assert!(
        spawner.running().is_empty(),
        "ledger must be empty after fast tasks: {:?}",
        spawner.running()
    );
}

/// 深 crate 的 `Spawner::current()`（tools/state/wiki 的收口入口）必须落入实例账目：
/// 外层 spawn 的任务内再 spawn，`running()` 能看到两层，结束后账目清零。
#[tokio::test(flavor = "multi_thread")]
async fn current_inside_spawned_task_uses_the_instance_ledger() {
    let spawner = Spawner::new();
    spawner.bind_context(context(
        fixture_env("ESCODE_TEST_LEDGER", "same-instance"),
        &spawner,
    ));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let outer = spawner.spawn("outer", async move {
        Spawner::current()
            .spawn_blocking("inner", move || {
                let _ = started_tx.send(());
                let _ = release_rx.recv();
            })
            .await
            .expect("inner task joins");
    });
    started_rx.await.expect("inner task started");
    assert_eq!(
        spawner.running(),
        vec!["outer".to_owned(), "inner".to_owned()],
        "deep-crate Spawner::current() must register in the same instance ledger"
    );
    release_tx.send(()).expect("release inner task");
    outer.await.expect("outer task joins");
    assert!(
        spawner.running().is_empty(),
        "ledger is empty after both tasks finish"
    );
}

#[tokio::test]
async fn instance_env_is_replayed_into_spawned_tasks() {
    let spawner = Spawner::new();
    spawner.bind_context(context(
        fixture_env("ESCODE_TEST_MARKER", "instance-42"),
        &spawner,
    ));
    let seen = spawner
        .spawn("read-env", async {
            crate::env::var("ESCODE_TEST_MARKER").ok()
        })
        .await
        .unwrap();
    assert_eq!(seen.as_deref(), Some("instance-42"));
}

#[tokio::test]
async fn blocking_tasks_see_instance_env_and_report_panics() {
    let spawner = Spawner::new();
    spawner.bind_context(context(
        fixture_env("ESCODE_TEST_BLOCKING", "yes"),
        &spawner,
    ));
    let seen = spawner
        .spawn_blocking("read-blocking-env", || {
            crate::env::var("ESCODE_TEST_BLOCKING").ok()
        })
        .await
        .unwrap();
    assert_eq!(seen.as_deref(), Some("yes"));
    let failed = spawner.spawn_blocking("boom-blocking", || panic!("detonation"));
    assert!(failed.await.unwrap_err().is_panic());
    assert_eq!(
        spawner.panics(),
        vec!["boom-blocking: detonation".to_owned()]
    );
}
