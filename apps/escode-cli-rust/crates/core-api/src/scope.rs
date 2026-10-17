//! 实例任务作用域 Spawner（rust-unified-host.md P1 #5、docs/specs/rust-u1-runtime-lib.md §3.1）。
//!
//! 实例树内 `tokio::spawn` / `spawn_blocking` 的唯一入口：登记在册（`running()` 残余清单）、
//! panic 上报（记录 + sink，随后 `resume_unwind` 保持 `JoinError::is_panic` 语义）、并把实例
//! 上下文（env 快照、实例级端口覆盖）重建到子任务——task-local 不随 spawn 继承，必须在这里重放。

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::env::{EnvSnapshot, INSTANCE, InstanceContext};

/// panic 上报出口：`(任务名, panic 消息)`。组合根接 file_log / 引擎事件。
pub type PanicSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct Spawner {
    context: OnceLock<Arc<InstanceContext>>,
    sink: OnceLock<PanicSink>,
    tasks: Arc<Mutex<BTreeMap<u64, TaskEntry>>>,
    next: AtomicU64,
    /// sink 缺席时的兜底记录（测试与诊断读取）。
    panics: Arc<Mutex<Vec<String>>>,
}

struct TaskEntry {
    name: &'static str,
    abort: tokio::task::AbortHandle,
}

impl Spawner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            context: OnceLock::new(),
            sink: OnceLock::new(),
            tasks: Arc::new(Mutex::new(BTreeMap::new())),
            next: AtomicU64::new(0),
            panics: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// 绑定实例上下文（embed 组装时调用一次；先到先得）。
    pub fn bind_context(self: &Arc<Self>, context: Arc<InstanceContext>) {
        let _ = self.context.set(context);
    }

    pub fn with_panic_sink(self: &Arc<Self>, sink: PanicSink) -> Arc<Self> {
        let _ = self.sink.set(sink);
        self.clone()
    }

    /// 实例树内返回实例 Spawner（子任务经 spawn 重建上下文后能再次解析到同一实例）；
    /// 否则新建临时作用域并绑当前快照——进程级装配路径（binary main、单测）各得独立账目。
    pub fn current() -> Arc<Self> {
        if let Ok(context) = INSTANCE.try_with(Clone::clone)
            && let Some(spawner) = context.spawner.upgrade()
        {
            return spawner;
        }
        let spawner = Self::new();
        let env = crate::env::current_snapshot()
            .unwrap_or_else(|| Arc::new(EnvSnapshot::capture_process()));
        spawner.bind_context(Arc::new(InstanceContext {
            env,
            spawner: Arc::downgrade(&spawner),
            usage_sink: OnceLock::new(),
            media_cache_root: OnceLock::new(),
        }));
        spawner
    }

    pub fn spawn<F>(
        self: &Arc<Self>,
        name: &'static str,
        fut: F,
    ) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let guard = TaskGuard {
            tasks: self.tasks.clone(),
            id,
        };
        let sink = self.sink.get().cloned();
        let panics = self.panics.clone();
        let context = self.context.get().cloned();
        let body = async move {
            let _guard = guard;
            match futures_util::FutureExt::catch_unwind(AssertUnwindSafe(fut)).await {
                Ok(output) => output,
                Err(payload) => {
                    // 重抛：await 方继续按 JoinError::panic 观测，既有错误语义不变。
                    let message = panic_message(&payload);
                    report_panic(&sink, &panics, name, &message);
                    std::panic::resume_unwind(payload);
                }
            }
        };
        let handle = tokio::spawn(async move {
            match context {
                Some(context) => INSTANCE.scope(context, body).await,
                None => body.await,
            }
        });
        self.register(id, name, handle.abort_handle());
        handle
    }

    pub fn spawn_blocking<F, T>(
        self: &Arc<Self>,
        name: &'static str,
        f: F,
    ) -> tokio::task::JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let guard = TaskGuard {
            tasks: self.tasks.clone(),
            id,
        };
        let sink = self.sink.get().cloned();
        let panics = self.panics.clone();
        let context = self.context.get().cloned();
        let body = move || {
            let _guard = guard;
            match std::panic::catch_unwind(AssertUnwindSafe(f)) {
                Ok(output) => output,
                Err(payload) => {
                    let message = panic_message(&payload);
                    report_panic(&sink, &panics, name, &message);
                    std::panic::resume_unwind(payload);
                }
            }
        };
        let handle = tokio::task::spawn_blocking(move || match context {
            Some(context) => INSTANCE.sync_scope(context, body),
            None => body(),
        });
        self.register(id, name, handle.abort_handle());
        handle
    }

    /// 中止全部在飞任务（实例任务结束时调用：与 ZCode-rs scope 语义一致——
    /// 实例随任务返回/失败/panic/中止而结束，残余任务一并中止）。
    pub fn abort_all(&self) {
        let entries: Vec<_> = self
            .tasks()
            .values()
            .map(|entry| entry.abort.clone())
            .collect();
        for abort in entries {
            abort.abort();
        }
    }

    fn register(&self, id: u64, name: &'static str, abort: tokio::task::AbortHandle) {
        self.tasks().insert(id, TaskEntry { name, abort });
    }

    /// 在飞任务名清单（关闭时列出残余）。
    pub fn running(&self) -> Vec<String> {
        self.tasks()
            .values()
            .map(|entry| entry.name.to_string())
            .collect()
    }

    /// 关闭宽限：等待任务收口，超时返回仍残余的名字。
    pub async fn drain(self: &Arc<Self>, grace: Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + grace;
        while !self.tasks().is_empty() {
            if tokio::time::Instant::now() >= deadline {
                return self.running();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Vec::new()
    }

    /// sink 缺席时兜底记录的 panic 消息（测试断言用）。
    pub fn panics(&self) -> Vec<String> {
        self.lock(&self.panics).clone()
    }

    fn tasks(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, TaskEntry>> {
        self.lock(&self.tasks)
    }

    /// 锁 poison 容忍（P0#4 同款：读路径不因写路径 panic 打崩）。
    fn lock<'a, T>(&self, cell: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        cell.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct TaskGuard {
    tasks: Arc<Mutex<BTreeMap<u64, TaskEntry>>>,
    id: u64,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        // future 结束（完成 / panic / abort）都会走这里，账目随之清零。
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_owned()
}

fn report_panic(sink: &Option<PanicSink>, panics: &Mutex<Vec<String>>, name: &str, message: &str) {
    if let Some(sink) = sink {
        sink(name, message);
        return;
    }
    panics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(format!("{name}: {message}"));
    eprintln!("escode task `{name}` panicked: {message}");
}

#[cfg(test)]
mod tests {
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
}
