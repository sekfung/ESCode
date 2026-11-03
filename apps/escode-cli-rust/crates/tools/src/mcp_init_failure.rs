//! MCP 初始化失败分类（docs/specs/rust-mcp-parity.md「stdio 握手前退出」）。

use rmcp::service::ClientInitializeError;

/// Node `openServerConnection` 对 stdio 的默认 failureKind 是 `process_start_failed`，
/// 只有版本协商类错误才改写成 `protocol_negotiation_failed`。子进程在握手前关闭
/// （Node 报 "Connection closed"）或 stdio 传输断开都属于进程启动失败。
pub(super) fn stdio(error: ClientInitializeError) -> anyhow::Error {
    fn process_failed(error: &ClientInitializeError) -> bool {
        match error {
            ClientInitializeError::ConnectionClosed(_)
            | ClientInitializeError::TransportError { .. } => true,
            ClientInitializeError::LegacyFallbackFailed { discover, fallback } => {
                process_failed(discover) || process_failed(fallback)
            }
            _ => false,
        }
    }
    let kind = if process_failed(&error) {
        "process_start_failed"
    } else {
        "protocol_negotiation_failed"
    };
    anyhow::Error::new(error).context(kind)
}

#[cfg(test)]
mod tests {
    use super::stdio;

    #[test]
    fn connection_closed_is_process_start_failed() {
        let error =
            rmcp::service::ClientInitializeError::ConnectionClosed("discover response".into());
        assert_eq!(stdio(error).to_string(), "process_start_failed");
    }

    #[test]
    fn protocol_error_remains_protocol_negotiation_failed() {
        let error = rmcp::service::ClientInitializeError::NoCompatibleProtocolVersion {
            client_supported: vec![],
            server_supported: vec![],
        };
        assert_eq!(stdio(error).to_string(), "protocol_negotiation_failed");
    }
}
