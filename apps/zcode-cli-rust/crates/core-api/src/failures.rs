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
/// 工具处理器失败（TS `ToolHandlerFailure`）：模型看到 `<tool_use_error>{message}</tool_use_error>`
/// （docs/specs/rust-file-tool-results.md）；其余错误按消息原文。
#[derive(Debug)]
pub struct ToolHandlerFailure(pub String);
impl std::fmt::Display for ToolHandlerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ToolHandlerFailure {}
