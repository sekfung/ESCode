//! Host 进程级日志出口（`docs/specs/rust-u2-host-skeleton.md` §3.2「Host 日志」）：
//! 日志与控制消息共用 stdio 单队列，日志满即丢、控制不丢。
//!
//! 与 CLI runtime 的 `escode-cli-host::file_log` 同构：进程级唯一 sink（`OnceLock`），
//! 下层的服务域（host-rpc / host-services）不持有控制通道句柄，只投递 `(scope, level, lines)`；
//! 二进制装配时把 sink 接到控制队列（`control_sink`）。
//!
//! 为什么不自建 `tracing`/`log` 门面：workspace 未引入这两个 crate，而 Host 的日志面本就是
//! 控制队列上的 typed 帧（`ControlMessage::LogBatch`，级别 + scope + 行），多一层门面只增加映射。
//! 级别语义沿用 AGENTS.md：`debug` 高频诊断、`info` 生命周期、`warn` 可恢复、`error` 不可恢复。
//!
//! 未安装 sink（单元测试、crate 被其它宿主嵌用）、sink 队列满或**已释放**时回退 stderr：
//! 诊断不丢，只是少了结构。sink 的生命周期由装配方掌管：`install_sink` 装上，
//! 收口时 `release_sink` 释放——sink 常持有控制队列的写句柄，不释放则队列永不关闭。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::LogLevel;

/// 日志 sink：由二进制决定投递去向。实现必须非阻塞——日志不能拖住业务路径。
pub type LogSink = std::sync::Arc<dyn Fn(LogBatch) + Send + Sync>;

/// 一条日志批次（`ControlMessage::LogBatch` 的有效载荷）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogBatch {
    pub scope: String,
    pub level: LogLevel,
    pub lines: Vec<String>,
}

/// 进程级 sink 槽。用 `Mutex` 而不是 `OnceLock`：sink 可以从这里**释放**
/// （见 [`release_sink`]）——sink 通常持有控制队列的写句柄，收口时不清掉，队列永不关闭。
static SINK: Mutex<Option<LogSink>> = Mutex::new(None);
/// sink 投递失败（队列满/已关闭）的批次数：日志可丢，但丢弃要可观测。
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// 安装 sink（进程内一次；重复调用忽略并返回 false）。
pub fn install_sink(sink: LogSink) -> bool {
    let mut slot = lock();
    if slot.is_some() {
        return false;
    }
    *slot = Some(sink);
    true
}

/// 释放 sink（收口路径）：之后日志回退 stderr。幂等；返回是否真的释放了。
pub fn release_sink() -> bool {
    lock().take().is_some()
}

fn lock() -> std::sync::MutexGuard<'static, Option<LogSink>> {
    SINK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 记一条日志。未安装 sink 或 sink 丢弃时回退 stderr。
pub fn log(scope: &str, level: LogLevel, lines: Vec<&str>) {
    if lines.is_empty() {
        return;
    }
    let batch = LogBatch {
        scope: scope.to_owned(),
        level,
        lines: lines.into_iter().map(str::to_owned).collect(),
    };
    // 取出回调再调用：不让 sink 的实现（可能阻塞在锁上）占着全局槽。
    let sink = lock().clone();
    match sink {
        Some(sink) => sink(batch),
        None => fallback(&batch),
    }
}

/// 同上，但投给**指定** sink（不走全局槽；测试用，避免测试之间争抢进程级状态）。
#[cfg(test)]
pub(crate) fn log_to(sink: &LogSink, scope: &str, level: LogLevel, lines: Vec<&str>) {
    if lines.is_empty() {
        return;
    }
    sink(LogBatch {
        scope: scope.to_owned(),
        level,
        lines: lines.into_iter().map(str::to_owned).collect(),
    });
}

/// sink 报告投递失败（队列满）：计数 + stderr 兜底。
pub fn record_dropped(batch: &LogBatch) {
    DROPPED.fetch_add(1, Ordering::Relaxed);
    fallback(batch);
}

/// 被丢弃的日志批次数（健康检查与测试用）。
pub fn dropped_batches() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

fn fallback(batch: &LogBatch) {
    for line in &batch.lines {
        eprintln!(
            "escode-host: [{}] {}: {line}",
            batch.scope,
            level_name(batch.level)
        );
    }
}

pub fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// 把控制队列的写者接成 sink：日志与控制共用**一条**有序队列（§3.2）。
/// `ControlWriter::log` 队满即丢并返回 false（控制消息走 `control()` 仍必达）。
pub fn control_sink(writer: crate::ControlWriter) -> LogSink {
    std::sync::Arc::new(move |batch: LogBatch| {
        if !writer.log(&batch.scope, batch.level, batch.lines.clone()) {
            record_dropped(&batch);
        }
    })
}

/// 一条 info 日志。
pub fn info(scope: &str, message: impl Into<String>) {
    log(scope, LogLevel::Info, vec![&message.into()]);
}

/// 一条 warn 日志。
pub fn warn(scope: &str, message: impl Into<String>) {
    log(scope, LogLevel::Warn, vec![&message.into()]);
}

/// 一条 error 日志。
pub fn error(scope: &str, message: impl Into<String>) {
    log(scope, LogLevel::Error, vec![&message.into()]);
}

/// 一条 debug 日志。
pub fn debug(scope: &str, message: impl Into<String>) {
    log(scope, LogLevel::Debug, vec![&message.into()]);
}

#[cfg(test)]
#[path = "logging_tests.rs"]
mod tests;
