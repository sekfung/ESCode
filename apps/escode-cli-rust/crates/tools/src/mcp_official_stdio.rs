//! 官方 stdio MCP 的身份载荷下发（docs/specs/rust-mcp-official-auth.md 第 3 期），对齐 TS
//! `ProcessTreeStdioClientTransport.send` + `mergeRequestMeta`：只改写 client 发出的请求与通知
//! （JSON-RPC response 没有 method，不伪造 params），宿主刚解析的载荷覆盖 `_meta` 中同名的残留值。
use super::mcp_official_client::Official;
use crate::domain::mcp_official_auth::META_KEY;
use rmcp::{model::ClientJsonRpcMessage, transport::async_rw::JsonRpcMessageCodecError};
use serde_json::{Map, Value};
use std::sync::Arc;

pub(super) async fn with_meta(
    official: Option<Arc<Official>>,
    message: ClientJsonRpcMessage,
) -> Result<Value, JsonRpcMessageCodecError> {
    let mut value = serde_json::to_value(&message)?;
    let Some(official) = official.filter(|_| value.get("method").is_some()) else {
        return Ok(value);
    };
    let payload = official.stdio_meta().await;
    if !value["params"].is_object() {
        value["params"] = Value::Object(Map::new());
    }
    if !value["params"]["_meta"].is_object() {
        value["params"]["_meta"] = Value::Object(Map::new());
    }
    value["params"]["_meta"][META_KEY] = payload;
    Ok(value)
}
