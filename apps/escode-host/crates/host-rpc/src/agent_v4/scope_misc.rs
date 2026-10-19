//! `escode-agent` 零散成员：子代理列表与 workspace runtime identity。

use serde_json::{Value, json};

use super::scope::Scope;
use crate::forward::RuntimePolicy;
use crate::session::RpcError;

impl Scope {
    pub(crate) async fn list_session_subagents(&self, params: &Value) -> Result<Value, RpcError> {
        let mut body = serde_json::Map::new();
        body.insert("sessionId".to_owned(), params["sessionId"].clone());
        if let Some(cursor) = params.get("endedCursor").filter(|value| !value.is_null()) {
            body.insert("endedCursor".to_owned(), cursor.clone());
        }
        body.insert(
            "endedLimit".to_owned(),
            params
                .get("endedLimit")
                .cloned()
                .unwrap_or_else(|| Value::from(20)),
        );
        let called = self
            .base
            .call(
                params,
                RuntimePolicy::StartIfNeeded,
                "session/subagents",
                Value::Object(body),
            )
            .await?;
        Ok(called.result)
    }

    /// `getWorkspaceRuntimeIdentity`：只观察现有实例，不启动 dormant workspace（TS existing-only）。
    pub(crate) fn runtime_identity(&self, params: &Value) -> Result<Value, RpcError> {
        let target = crate::forward::runtime_target(params.clone())?;
        let Some(instance) = self.base.instances.live(&target.identity) else {
            return Err(RpcError::new(
                "Error",
                "ESCode agent runtime identity is unavailable.",
            ));
        };
        Ok(json!({
            "generation": instance.generation,
            "identity": instance.identity,
            "workspaceKey": instance.identity,
        }))
    }
}
