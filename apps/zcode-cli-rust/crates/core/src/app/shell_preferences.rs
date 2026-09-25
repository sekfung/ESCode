//! 会话运行时偏好：Engine 是唯一 owner，按（会话, scope）向 Host 请求一次
//! `session/requestRuntimePreferences` 并缓存完整结果，对应 TS `resolveSessionStartupPreferences`：
//! - `runtime-materialization`：会话首轮前，读取 `memoryEnabled`（docs/specs/rust-project-memory.md）；
//! - `user-execution`：首个 Bash 前，读取 `integratedTerminalShell`（docs/specs/rust-shell-selection.md）。
//!
//! 并发的等待方共用同一个 in-flight 请求，回包后统一答复。
use super::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::oneshot;

type Key = (String, &'static str);
type Waiter = Box<dyn FnOnce(&Value) + Send + Sync>;

#[derive(Default)]
pub(super) struct ShellPreferences {
    cache: BTreeMap<Key, Value>,
    waiters: BTreeMap<Key, Vec<Waiter>>,
    requests: BTreeMap<String, Key>,
}

impl Engine {
    fn request_runtime_preferences(&mut self, session: &str, scope: &'static str, waiter: Waiter) {
        let key = (session.to_owned(), scope);
        if let Some(value) = self.shell.cache.get(&key) {
            waiter(value);
            return;
        }
        let waiters = self.shell.waiters.entry(key.clone()).or_default();
        waiters.push(waiter);
        if waiters.len() > 1 {
            return;
        }
        let request_id = format!("rust-runtime-prefs-{}", self.clock.id());
        self.shell.requests.insert(request_id.clone(), key);
        self.outbox.push(json!({
            "id": request_id,
            "method": "session/requestRuntimePreferences",
            "params": {"sessionId": session, "scope": scope},
        }));
    }

    pub(super) fn request_shell_preference(
        &mut self,
        session: &str,
        reply: oneshot::Sender<Option<Value>>,
    ) {
        self.request_runtime_preferences(
            session,
            "user-execution",
            Box::new(move |result| {
                let value = result
                    .get("integratedTerminalShell")
                    .filter(|v| !v.is_null())
                    .cloned();
                let _ = reply.send(value);
            }),
        );
    }

    /// Host Memory Settings 总开关；缺省与旧 Host（错误回包归一为 Null）均为关闭（TS memoryEnabled 默认 false）。
    /// 回复同时带上本会话已缓存的记忆（启用时复用，避免会话内重复解析）。
    pub(super) fn request_memory_preference(
        &mut self,
        session: &str,
        reply: oneshot::Sender<(bool, Option<crate::contract::ProjectMemory>)>,
        cached: Option<crate::contract::ProjectMemory>,
    ) {
        self.request_runtime_preferences(
            session,
            "runtime-materialization",
            Box::new(move |result| {
                let _ = reply.send((result["memoryEnabled"] == true, cached));
            }),
        );
    }
    /// 会话关闭后下次物化重新向 Host 请求（Memory Settings 变化随新物化生效）。
    pub(super) fn forget_runtime_preferences(&mut self, session: &str) {
        self.shell.cache.retain(|(s, _), _| s != session);
    }

    /// 非本模块发出的请求直接忽略。错误回包（旧 Host 的 -32601 等）在传输层归一为 Null，
    /// 与 TS 兼容回退一致按缺省处理。
    pub(super) fn resolve_shell_preference(&mut self, request_id: &str, result: &Value) {
        let Some(key) = self.shell.requests.remove(request_id) else {
            return;
        };
        self.shell.cache.insert(key.clone(), result.clone());
        for waiter in self.shell.waiters.remove(&key).unwrap_or_default() {
            waiter(result);
        }
    }
}
