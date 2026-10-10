//! 工具处理器失败（TS `ToolHandlerFailure`）：模型看到 `<tool_use_error>{message}</tool_use_error>`，
//! 其余错误按消息原文（docs/specs/rust-file-tool-results.md）。

#[derive(Debug)]
pub struct ToolHandlerFailure(pub String);
impl std::fmt::Display for ToolHandlerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ToolHandlerFailure {}

/// TS `projectExecutionErrorPayload` 的 `sanitizeText`：普通工具错误进入模型前折叠空白、去首尾空白，
/// 超过 500 个 UTF-16 码元时截为 497 加 `...`。
pub fn plain_error_text(message: &str) -> String {
    let compact = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let units: Vec<u16> = compact.encode_utf16().collect();
    if units.len() <= 500 {
        return compact;
    }
    format!("{}...", String::from_utf16_lossy(&units[..497]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn collapses_whitespace_and_bounds_length() {
        assert_eq!(super::plain_error_text("[\n  {\n    \"a\": 1\n  }\n]"), "[ { \"a\": 1 } ]");
        let long = "x".repeat(600);
        assert_eq!(super::plain_error_text(&long), format!("{}...", "x".repeat(497)));
    }
}
