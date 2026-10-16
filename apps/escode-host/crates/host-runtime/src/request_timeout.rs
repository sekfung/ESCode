//! 请求超时的类型化错误（与「实例已关闭」区分，供 stale 实例恢复使用）。

use std::time::Duration;

/// 请求在期限内没有应答（与「实例已关闭」区分：调用方据此决定是否回收 stale 实例并重试）。
#[derive(Debug)]
pub struct RequestTimeout {
    pub method: String,
    pub timeout: Duration,
}

impl std::fmt::Display for RequestTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "runtime did not answer `{}` within {:?}",
            self.method, self.timeout
        )
    }
}

impl std::error::Error for RequestTimeout {}
