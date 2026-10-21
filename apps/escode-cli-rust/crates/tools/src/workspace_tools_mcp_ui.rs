//! `WorkspaceTools` 的 MCP Apps 方法分派（`ToolPort::mcp_ui` 的实现细节）。
//!
//! 拆出 `tools.rs` 以守住 400 有效行门禁；状态与语义都在 `crate::mcp_ui` / `mcp_hub_ui`。
use super::WorkspaceTools;
use anyhow::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

impl WorkspaceTools {
    pub(crate) async fn mcp_ui_dispatch(&self, method: &str, params: &Value) -> Result<Value> {
        let cancel = CancellationToken::new();
        match method {
            "mcp/uiOpenInstance" => self.mcp.ui_open(params).await,
            "mcp/uiValidateInstance" => self.mcp.ui_validate(params).await,
            "mcp/uiCloseInstance" => self.mcp.ui_close(params).await,
            "mcp/readResource" => self.mcp.ui_read_host_resource(params, &cancel).await,
            "mcp/uiReadResource" => self.mcp.ui_read_page_resource(params, &cancel).await,
            "mcp/uiListResources" => self.mcp.ui_list_resources(params, false, &cancel).await,
            "mcp/uiListResourceTemplates" => {
                self.mcp.ui_list_resources(params, true, &cancel).await
            }
            "mcp/uiSubscribeResource" => self.mcp.ui_subscribe(params, true, &cancel).await,
            "mcp/uiUnsubscribeResource" => self.mcp.ui_subscribe(params, false, &cancel).await,
            "mcp/uiCallTool" => self.mcp.ui_call_tool(params, &cancel).await,
            "mcp/uiCancelCall" => self.mcp.ui_cancel_call(params).await,
            other => anyhow::bail!("Unsupported method: {other}"),
        }
    }
}
