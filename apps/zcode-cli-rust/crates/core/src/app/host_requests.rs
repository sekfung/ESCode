//! 工具经会话 owner 发起的 Host 反向请求（automation/* 等），按请求 id 路由应答。
//! 见 docs/specs/rust-cron.md。

use super::Engine;
use crate::contract::HostReply;
use serde_json::{Value, json};

impl Engine {
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
