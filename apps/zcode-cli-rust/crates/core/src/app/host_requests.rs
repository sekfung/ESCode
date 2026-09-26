//! 工具经会话 owner 发起的 Host 反向请求（automation/* 等），按请求 id 路由应答。
//! 见 docs/specs/rust-cron.md。

use super::Engine;
use crate::contract::{Event, HostReply};
use serde_json::{Value, json};

impl Engine {
    /// turn 收尾后通知工具层（浏览器 turnEnded，只覆盖 runtime 自己操作过的 browser）；失败不影响已完成的 turn（TS 同）。
    pub(super) fn notify_turn_ended(&self, id: &str, turn: &str) {
        let (tools, id, turn) = (self.tools.clone(), id.to_owned(), turn.to_owned());
        tokio::spawn(async move { tools.turn_ended(&id, &turn).await });
    }
    /// 工具层 Host 通道不依附会话（如 mcp/list 期间的官方 MCP 身份头），直接转发，应答按请求 id 路由。
    pub(super) fn host_channel_event(&mut self, event: Event) {
        if let Event::HostRequest { method, params, reply } = event {
            self.request_host(method, params, reply);
        }
    }
    pub(super) fn request_host(&mut self, method: String, params: Value, reply: HostReply) {
        let request_id = format!("rust-host-{}", self.clock.id());
        self.host_requests.insert(request_id.clone(), reply);
        self.outbox
            .push(json!({"id": request_id, "method": method, "params": params}));
    }

    /// Host 应答：先匹配工具反向请求（错误保留 code/message，结果保留原始文本），再依次交给鉴权与 shell 偏好。
    pub(super) fn resolve_response(
        &mut self,
        id: &str,
        result: Value,
        error: Option<Value>,
        raw_result: Option<String>,
    ) {
        if let Some(reply) = self.host_requests.remove(id) {
            let _ = reply.send(match error {
                Some(error) => Err((
                    error["code"].as_i64().unwrap_or(0),
                    error["message"].as_str().unwrap_or_default().to_owned(),
                )),
                None => Ok(raw_result.unwrap_or_else(|| result.to_string())),
            });
        } else if let Some((_, _, _, reply)) = self.auth.remove(id) {
            let _ = reply.send(result);
        } else {
            self.resolve_shell_preference(id, &result);
        }
    }
}

impl Engine {
    /// 会话 owner 代为完成的请求类事件（Host 反向请求、存储读取、偏好、记忆、鉴权），不改变会话投影。
    pub(super) fn owner_request(&mut self, id: &str, run_id: &str, turn: &str, event: Event) {
        match event {
            Event::HostRequest {
                method,
                params,
                reply,
            } => self.request_host(method, params, reply),
            Event::SessionContext { id: target, reply } => {
                // 存储读取不阻塞会话 actor。
                let store = self.store.clone();
                tokio::spawn(async move {
                    let _ = reply.send(store.session_context(&target).await);
                });
            }
            Event::ShellPreference { reply } => self.request_shell_preference(id, reply),
            Event::MemoryPreference { reply } => {
                let cached = self.memory_prompts.get(id).cloned();
                self.request_memory_preference(id, reply, cached);
            }
            Event::MemoryResolved(memory) => {
                self.memory_prompts.insert(id.to_owned(), memory);
            }
            Event::MemoryExtract(snapshot) => self.schedule_memory(id, *snapshot),
            Event::RequestAuth {
                provider,
                selection,
                access,
                reply,
            } if !reply.is_closed() => {
                let request_id = format!("rust-auth-{}", self.clock.id());
                let workspace = json!({"workspaceKey":self.workspace,"workspacePath":self.workspace_path,"workspaceIdentity":self.workspace});
                let params = json!({"requestId":request_id,"sessionId":id,"turnId":turn,"workspace":workspace,"providerId":provider,"modelSelection":selection,"accountAccess":access,"reason":"model-request"});
                self.auth.insert(
                    request_id.clone(),
                    (id.to_owned(), run_id.to_owned(), workspace, reply),
                );
                self.outbox.push(json!({"id":request_id,"method":"interaction/requestProviderRuntimeHeaders","params":params}));
            }
            _ => {}
        }
    }
}
