//! MCP hub 的浏览器与工具卡部分（docs/specs/rust-browser-use.md 第 2–3 期）：broker 生命周期、轮尾截图、
//! 会话中 MCP 工具的 `mcp_tool` 工具卡。
use super::{Hub, binding::Binding};
use serde_json::Value;
use std::sync::Arc;

impl Binding {
    /// 工具卡；`output` 为本次 `tools/call` 的原始结果（ToolStart 阶段还没有结果）。
    pub(super) fn display_with_output(&self, output: Option<&Value>) -> Option<Value> {
        let description = self.definition["function"]["description"].as_str();
        let mut display =
            crate::domain::tool_display::mcp_tool(&self.server, &self.original, description)?;
        // UI 的 pluginUiRowPolicy 要求 `display.ui !== undefined` 才建卡；缺这段投影时
        // MCP App 卡片在 Rust runtime 下完全不出现（对齐 TS createMcpToolDisplay）。
        if let Some(ui) = self.ui.as_ref().and_then(|descriptor| {
            crate::domain::mcp_tool_ui::display_ui(descriptor, &self.plugin_id, output)
        }) {
            display["ui"] = ui;
        }
        Some(display)
    }
    pub(super) fn display(&self) -> Option<Value> {
        self.display_with_output(None)
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
    /// 会话中 MCP 工具的工具卡（ToolStart 时投影到行级 display）。
    pub fn tool(&self, session: &str, name: &str) -> Option<crate::contract::McpTool> {
        let state = self.state.read().unwrap();
        let binding = state
            .bindings
            .get(session)?
            .iter()
            .find(|b| b.name == name)?;
        Some(crate::contract::McpTool {
            display: binding.display(),
        })
    }
}
