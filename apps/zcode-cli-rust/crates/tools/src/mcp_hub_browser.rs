//! MCP hub 的浏览器与工具卡部分（docs/specs/rust-browser-use.md 第 2–3 期）：broker 生命周期、轮尾截图、
//! 会话中 MCP 工具的 `mcp_tool` 工具卡。
use super::{Binding, Hub};
use serde_json::Value;
use std::sync::Arc;

impl Binding {
    pub(super) fn display(&self) -> Option<Value> {
        let description = self.definition["function"]["description"].as_str();
        crate::domain::tool_display::mcp_tool(&self.server, &self.original, description)
    }
}
impl Hub {
    pub(super) fn broker(&self) -> Option<Arc<crate::browser_broker::Broker>> {
        self.broker
            .get_or_init(|| crate::browser_broker::Broker::start(self.host.clone()))
            .clone()
    }
    /// 成功轮次收尾的浏览器截图卡（TS appendBrowserTurnScreenshot）。
    pub async fn browser_turn_screenshot(&self, session: &str, turn: &str) -> Option<Value> {
        let broker = self.broker.get()?.clone()?;
        broker.turn_screenshot(session, turn).await
    }
    /// turn 结束或会话关闭时的浏览器生命周期（TS BrowserControlPort.turnEnded / closeSession）。
    pub async fn browser_lifecycle(&self, session: &str, turn: Option<&str>, close: bool) {
        if let Some(Some(broker)) = self.broker.get() {
            broker.lifecycle(session, turn, close).await;
        }
    }
    /// 会话中 MCP 工具的工具卡（ToolStart 时投影到行级 display）与 inputSchema（执行前入参校验，
    /// docs/specs/rust-tool-input-validation.md）。
    pub fn tool(&self, session: &str, name: &str) -> Option<crate::contract::McpTool> {
        let state = self.state.read().unwrap();
        let binding = state.bindings.get(session)?.iter().find(|b| b.name == name)?;
        Some(crate::contract::McpTool {
            display: binding.display(),
            input_schema: binding.definition["function"]["parameters"].clone(),
        })
    }
}
