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
    /// 动态工作流灰度门（`workspace/updateDynamicWorkflowPolicy`，docs/specs/rust-dynamic-workflow.md
    /// 第 1 期）：进程级结论 + 各会话固化值。
    pub(super) dynamic_workflow: crate::domain::dynamic_workflow::Policy,
    /// 单次执行作用域材料（modelExecution，只在内存）。
    pub(super) executions: super::model_execution::Executions,
}

impl ShellPreferences {
    /// 已缓存的（会话, scope）偏好；未请求过即 `None`。
    pub(super) fn cached(&self, session: &str, scope: &'static str) -> Option<&Value> {
        self.cache.get(&(session.to_owned(), scope))
    }
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

    /// 会话创建期发起 `runtime-materialization` 偏好请求（不等待回包）。
    /// 差分账本 C-1/C-2（docs/reports/rust-difference-ledger-2026-10-14.md）：TS 在物化期用宿主本地
    /// 回调取 memoryEnabled；Rust 必须走一次协议往返，且引擎 outbox 在响应帧之后才冲刷，所以创建响应
    /// 本身拿不到该值。在创建期发起可保证回合开始前回包已入缓存：遥测事实（UI 埋点消费
    /// fact.memoryEnabled）与后续命令 ack 都带上该字段。旧 Host/异常回包在传输层归一为 Null
    /// （shared schema memoryEnabled 默认 false，与 memory 行为一致）。
    pub(super) fn request_materialization(&mut self, session: &str) {
        if self
            .shell
            .cached(session, "runtime-materialization")
            .is_some()
        {
            return;
        }
        self.request_runtime_preferences(session, "runtime-materialization", Box::new(|_| {}));
    }

    pub(super) fn request_shell_preference(
        &mut self,
        session: &str,
        reply: oneshot::Sender<Option<Value>>,
    ) {
        // TS 子代理继承父会话已解析的 shell 选择（subagent.ts 把父的 bashShellSelection 传给 child），
        // 不再以自己的 sessionId 请求 user-execution。子会话沿 parent 链上溯到根会话，请求与缓存
        // 都挂在根上：Rust 是首个 Bash 前懒解析，而 TS 在会话物化时就解析，所以子会话先跑 Bash 时
        // 必须替根会话取一次，否则会退化成自动探测、与 App 里父会话的显式选择不一致。
        let owner = self.shell_owner_session(session);
        self.request_runtime_preferences(
            &owner,
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

    /// `user-execution` 偏好的归属会话：子会话归其根会话（TS 的继承语义），主会话归自己。
    pub(super) fn shell_owner_session(&self, session: &str) -> String {
        let mut current = session;
        // 深度上限与子代理嵌套上限一致，父链异常时不至于死循环。
        for _ in 0..8 {
            match self
                .sessions
                .get(current)
                .and_then(|s| s.parent_id.as_deref())
            {
                Some(parent) => current = parent,
                None => break,
            }
        }
        current.to_owned()
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
