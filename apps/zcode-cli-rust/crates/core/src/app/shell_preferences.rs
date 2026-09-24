//! 会话终端 shell 偏好：Engine 是唯一 owner，按会话向 Host 请求一次
//! `session/requestRuntimePreferences{scope:"user-execution"}` 并缓存，
//! 对应 TS `resolveSessionStartupPreferences` 的 `resolveInitialBashShellSelection`。
//! 并发的首批 Bash 共用同一个 in-flight 请求，回包后统一答复。
use super::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::oneshot;

type Reply = oneshot::Sender<Option<Value>>;

#[derive(Default)]
pub(super) struct ShellPreferences {
    cache: BTreeMap<String, Option<Value>>,
    waiters: BTreeMap<String, Vec<Reply>>,
    requests: BTreeMap<String, String>,
}

impl Engine {
    pub(super) fn request_shell_preference(&mut self, session: &str, reply: Reply) {
        if let Some(value) = self.shell.cache.get(session) {
            let _ = reply.send(value.clone());
            return;
        }
        let waiters = self.shell.waiters.entry(session.to_owned()).or_default();
        waiters.push(reply);
        if waiters.len() > 1 {
            return;
        }
        let request_id = format!("rust-runtime-prefs-{}", self.clock.id());
        self.shell
            .requests
            .insert(request_id.clone(), session.to_owned());
        self.outbox.push(json!({
            "id": request_id,
            "method": "session/requestRuntimePreferences",
            "params": {"sessionId": session, "scope": "user-execution"},
        }));
    }

    /// 非本模块发出的请求直接忽略。错误回包（旧 Host 的 -32601 等）在传输层归一为 Null，
    /// 与 TS 兼容回退一致按自动探测处理。
    pub(super) fn resolve_shell_preference(&mut self, request_id: &str, result: &Value) {
        let Some(session) = self.shell.requests.remove(request_id) else {
            return;
        };
        let value = result
            .get("integratedTerminalShell")
            .filter(|v| !v.is_null())
            .cloned();
        self.shell.cache.insert(session.clone(), value.clone());
        for reply in self.shell.waiters.remove(&session).unwrap_or_default() {
            let _ = reply.send(value.clone());
        }
    }
}
