//! `Connection` 的 MCP 资源面（只读读取与订阅），拆分子模块以满足 400 行门禁。

use super::Connection;
use anyhow::Result;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

impl Connection {
    /// MCP Apps 只读资源面（docs/specs/rust-mcp-ui.md §4 P0）：分页游标透传，结果原样交给调用方投影。
    pub async fn read_resource(&self, uri: &str, cancel: &CancellationToken) -> Result<Value> {
        self.request("resources/read", json!({"uri":uri}), cancel)
            .await
    }
    pub async fn list_resources(
        &self,
        cursor: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let params = cursor.map_or_else(|| json!({}), |cursor| json!({"cursor":cursor}));
        self.request("resources/list", params, cancel).await
    }
    pub async fn list_resource_templates(
        &self,
        cursor: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let params = cursor.map_or_else(|| json!({}), |cursor| json!({"cursor":cursor}));
        self.request("resources/templates/list", params, cancel)
            .await
    }
    /// MCP 资源订阅（`mcp/uiSubscribeResource` 的传输侧）；引用计数在 Hub 的登记表。
    pub async fn subscribe_resource(&self, uri: &str, cancel: &CancellationToken) -> Result<Value> {
        self.request("resources/subscribe", json!({"uri":uri}), cancel)
            .await
    }
    pub async fn unsubscribe_resource(
        &self,
        uri: &str,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.request("resources/unsubscribe", json!({"uri":uri}), cancel)
            .await
    }
}
