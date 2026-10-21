//! 运行时收口用的错误标记类型。

#[derive(Debug)]
pub struct StorageCommitFailure;
impl std::fmt::Display for StorageCommitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("fault.storage.commit")
    }
}
impl std::error::Error for StorageCommitFailure {}
#[derive(Debug)]
pub struct ProcessCleanupFailure;
impl std::fmt::Display for ProcessCleanupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("fault.runtime.processCleanup")
    }
}
impl std::error::Error for ProcessCleanupFailure {}
pub use escode_cli_domain::tool_failure::ToolHandlerFailure;

/// 带 JSON-RPC `code` 的请求级失败（MCP Apps 采样等需要区分 -32602 / -32000 的方法）。
#[derive(Debug)]
pub struct RpcFailure {
    pub code: i32,
    pub message: String,
}
impl RpcFailure {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RpcFailure {}
