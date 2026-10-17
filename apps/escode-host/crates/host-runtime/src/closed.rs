//! 实例读端关闭的类型化错误（docs/specs/rust-u2-host-resilience.md §2.4）。

/// 实例读端已关闭（崩溃、退出或收口）时，在飞请求与之后发出的请求都以它失败。
///
/// 文案逐字对齐 TS Host：agent 子进程 stdout 结束时 stdio transport 给出 `reason: "stdout_closed"`
/// （`escodeStdioTransport.ts`），协议客户端以 `ESCode agent transport closed: <reason>` 拒绝全部在飞请求
/// （`escodeProtocolClient.ts`）。Rust Host 的读端关闭就是 stdout 结束的等价物。
#[derive(Debug)]
pub struct RuntimeClosed {
    /// 失败的请求方法（只用于诊断，不进文案）。
    pub method: String,
}

/// Node 形状的文案（Renderer 可见）。
pub const RUNTIME_CLOSED_MESSAGE: &str = "ESCode agent transport closed: stdout_closed";

impl std::fmt::Display for RuntimeClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(RUNTIME_CLOSED_MESSAGE)
    }
}

impl std::error::Error for RuntimeClosed {}
