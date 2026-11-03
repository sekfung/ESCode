//! MCP 初始化失败分类（docs/specs/rust-mcp-parity.md「stdio 握手前退出」）。
//!
//! Node `openServerConnection` 先按传输设置默认 failureKind（stdio = process_start_failed，
//! http/sse = network_unreachable），只有版本协商类错误才改写为 protocol_negotiation_failed。
//! rmcp 的错误类型更细，这里按同一语义归一，避免把进程退出或普通 JSON-RPC 错误误报成协商失败。

use rmcp::{model::ProtocolVersion, service::ClientInitializeError};

#[derive(Clone, Copy)]
pub(super) enum DefaultFailure {
    ProcessStart,
    NetworkUnreachable,
}

impl DefaultFailure {
    fn kind(self) -> &'static str {
        match self {
            Self::ProcessStart => "process_start_failed",
            Self::NetworkUnreachable => "network_unreachable",
        }
    }
}

pub(super) fn classify(
    error: ClientInitializeError,
    default: DefaultFailure,
    probing: bool,
) -> anyhow::Error {
    let kind = kind(&error, default, probing);
    anyhow::Error::new(error).context(kind)
}

fn kind(error: &ClientInitializeError, default: DefaultFailure, probing: bool) -> &'static str {
    match error {
        ClientInitializeError::NoCompatibleProtocolVersion { .. }
        | ClientInitializeError::NoPreferredProtocolVersion
        | ClientInitializeError::ConflictInitResponseId(..)
        | ClientInitializeError::UncorrelatedErrorResponse { .. } => "protocol_negotiation_failed",
        // Auto 回落后 Node 抛的是 fallback 错误；discover 侧的错误只决定是否回落。
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            kind(fallback, default, false)
        }
        // 只有探测阶段的异常应答属于 Node 的 EraNegotiationFailed；legacy initialize 的同类
        // 错误在 Node 是普通 McpError/ZodError，落到传输默认值。
        ClientInitializeError::ExpectedInitResponse(_)
        | ClientInitializeError::ExpectedInitResult(_) => {
            if probing {
                "protocol_negotiation_failed"
            } else {
                default.kind()
            }
        }
        // HTTP/SSE 的 discover 传输失败对应 Node 的探测硬失败；stdio 的传输失败是子进程问题。
        ClientInitializeError::TransportError { .. }
            if probing && !matches!(default, DefaultFailure::ProcessStart) =>
        {
            "protocol_negotiation_failed"
        }
        ClientInitializeError::ConnectionClosed(_)
        | ClientInitializeError::TransportError { .. }
        | ClientInitializeError::JsonRpcError(_)
        | ClientInitializeError::Cancelled => default.kind(),
        _ => default.kind(),
    }
}

/// Node legacy `initialize` 只接受 pre-2026 的已知版本（`legacyProtocolVersions`）；
/// 现代协商也只可能落在 `KNOWN_VERSIONS` 内。未知版本不能被当成连接成功。
pub(super) fn unsupported_version(
    version: &ProtocolVersion,
    legacy_only: bool,
    transport: &str,
) -> Option<anyhow::Error> {
    // rmcp 3.4 没有 has_initialize；Node 的 legacyProtocolVersions 是 pre-2026-07-28。
    let supported = ProtocolVersion::KNOWN_VERSIONS.contains(version)
        && (!legacy_only || version.as_str() < ProtocolVersion::V_2026_07_28.as_str());
    if supported {
        return None;
    }
    let kind = if transport == "stdio" {
        "process_start_failed"
    } else {
        "network_unreachable"
    };
    Some(anyhow::anyhow!("{kind}"))
}

#[cfg(test)]
mod tests {
    use super::{DefaultFailure, classify, unsupported_version};
    use rmcp::{model::ProtocolVersion, service::ClientInitializeError};

    fn json_rpc_error() -> ClientInitializeError {
        ClientInitializeError::JsonRpcError(rmcp::model::ErrorData::new(
            rmcp::model::ErrorCode(-32000),
            "denied",
            None,
        ))
    }

    fn fallback(fallback: ClientInitializeError) -> ClientInitializeError {
        ClientInitializeError::LegacyFallbackFailed {
            discover: Box::new(ClientInitializeError::NoCompatibleProtocolVersion {
                client_supported: vec![],
                server_supported: vec![],
            }),
            fallback: Box::new(fallback),
        }
    }

    #[test]
    fn stdio_fallback_json_rpc_error_is_process_start_failed() {
        assert_eq!(
            classify(
                fallback(json_rpc_error()),
                DefaultFailure::ProcessStart,
                true
            )
            .to_string(),
            "process_start_failed"
        );
    }

    #[test]
    fn stdio_fallback_invalid_result_is_process_start_failed() {
        assert_eq!(
            classify(
                fallback(ClientInitializeError::ExpectedInitResult(None)),
                DefaultFailure::ProcessStart,
                true,
            )
            .to_string(),
            "process_start_failed"
        );
    }

    #[test]
    fn stdio_probe_invalid_response_remains_protocol_negotiation_failed() {
        assert_eq!(
            classify(
                ClientInitializeError::ExpectedInitResult(None),
                DefaultFailure::ProcessStart,
                true,
            )
            .to_string(),
            "protocol_negotiation_failed"
        );
    }

    #[test]
    fn http_fallback_json_rpc_error_is_network_unreachable() {
        assert_eq!(
            classify(
                fallback(json_rpc_error()),
                DefaultFailure::NetworkUnreachable,
                true,
            )
            .to_string(),
            "network_unreachable"
        );
    }

    #[test]
    fn http_connection_failure_during_probe_is_protocol_negotiation_failed() {
        assert_eq!(
            classify(
                ClientInitializeError::ConnectionClosed("discover".into()),
                DefaultFailure::NetworkUnreachable,
                true,
            )
            .to_string(),
            "network_unreachable"
        );
    }

    #[test]
    fn legacy_initialize_json_rpc_error_is_network_unreachable() {
        assert_eq!(
            classify(json_rpc_error(), DefaultFailure::NetworkUnreachable, false,).to_string(),
            "network_unreachable"
        );
    }

    #[test]
    fn negotiation_errors_stay_protocol_negotiation_failed() {
        assert_eq!(
            classify(
                ClientInitializeError::NoCompatibleProtocolVersion {
                    client_supported: vec![],
                    server_supported: vec![],
                },
                DefaultFailure::NetworkUnreachable,
                false,
            )
            .to_string(),
            "protocol_negotiation_failed"
        );
    }

    #[test]
    fn unknown_version_is_rejected_with_transport_default() {
        let unknown: ProtocolVersion = serde_json::from_str("\"1999-01-01\"").unwrap();
        assert_eq!(
            unsupported_version(&unknown, false, "stdio")
                .unwrap()
                .to_string(),
            "process_start_failed"
        );
        assert_eq!(
            unsupported_version(&unknown, false, "http")
                .unwrap()
                .to_string(),
            "network_unreachable"
        );
    }

    #[test]
    fn legacy_handshake_rejects_modern_version_but_auto_accepts_it() {
        assert!(unsupported_version(&ProtocolVersion::V_2026_07_28, true, "stdio").is_some());
        assert!(unsupported_version(&ProtocolVersion::V_2026_07_28, false, "stdio").is_none());
        assert!(unsupported_version(&ProtocolVersion::V_2025_11_25, true, "stdio").is_none());
    }
}
