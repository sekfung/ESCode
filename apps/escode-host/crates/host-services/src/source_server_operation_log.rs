//! 连接/更新操作日志载荷（TS `sourceServerOperationLogSchema`）。

use serde::Serialize;

/// 连接/更新操作日志（TS `sourceServerOperationLogSchema`）。
///
/// `generation` 不属于日志载荷；订阅端按到达顺序消费，registry 只保留最近若干条。
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionOperationLog {
    pub profile_id: String,
    pub operation: &'static str,
    pub phase: String,
    pub level: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    pub timestamp: u64,
}

impl ConnectionOperationLog {
    pub fn new(
        profile_id: &str,
        operation: &'static str,
        phase: impl Into<String>,
        level: &'static str,
        message: impl Into<String>,
        timestamp: u64,
    ) -> Self {
        Self {
            profile_id: profile_id.to_owned(),
            operation,
            phase: phase.into(),
            level,
            message: message.into(),
            progress: None,
            timestamp,
        }
    }
}
