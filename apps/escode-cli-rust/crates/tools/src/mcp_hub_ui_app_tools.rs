//! Hub 侧的 App-Provided Tools：登记、认领/回传、模型工具面与信箱投递。
//!
//! 登记表是会话 live-only 状态；模型调用先发 `pluginUi.appToolCall` 再等页面 claim，
//! Host/Engine 不持有第二个待执行队列（docs/specs/rust-mcp-ui.md §4 P3）。
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::Hub;
use crate::contract::ToolOutput;
use crate::mcp_ui::{await_result, to_output};

impl Hub {
    pub(crate) fn app_tool_register(&self, params: &Value) -> Result<Value> {
        self.ui_validate_scope(params)?;
        let session = session_id(params)?;
        self.ui().app_tools.register(session, params)
    }

    pub(crate) fn app_tool_unregister(&self, params: &Value) -> Value {
        let Some(session) = params["sessionId"].as_str() else {
            return json!({ "removed": 0 });
        };
        let removed = self.ui().app_tools.unregister(session, params);
        json!({ "removed": removed })
    }

    pub(crate) fn app_tool_claim(&self, params: &Value) -> Result<Value> {
        self.ui_validate_scope(params)?;
        let session = session_id(params)?;
        Ok(json!({ "accepted": self.ui().app_tools.claim(session, params) }))
    }

    pub(crate) fn app_tool_resolve(&self, params: &Value) -> Result<Value> {
        self.ui_validate_scope(params)?;
        let session = session_id(params)?;
        Ok(json!({ "accepted": self.ui().app_tools.resolve(session, params) }))
    }

    pub(crate) fn app_tool_definitions(&self, session: &str) -> Vec<Value> {
        self.ui().app_tools.definitions(session)
    }

    /// 模型发起页面工具调用：登记待认领调用、发 live-only 信箱 delta，再等 claim + 执行。
    pub(crate) async fn app_tool_call(
        &self,
        session: &str,
        name: &str,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let (call_id, delta, claim, settle) = {
            let mut ui = self.ui();
            ui.app_tools.prepare_call(session, name, args)?
        };
        let server_name = delta["serverName"].as_str().unwrap_or_default().to_owned();
        if let Some(live) = self.live.get() {
            let _ = live.send((session.to_owned(), delta));
        } else {
            // 没有 live 通道时页面收不到信箱，直接按“页面未运行”失败，不等满 5 秒。
            self.ui().app_tools.finish_call(session, &call_id);
            anyhow::bail!("The {server_name} app page is not running");
        }
        let result = await_result(claim, settle, &server_name, cancel).await;
        self.ui().app_tools.finish_call(session, &call_id);
        Ok(to_output(result?))
    }
}

fn session_id(params: &Value) -> Result<&str> {
    params["sessionId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .context("sessionId is required")
}
