//! `logging` 的行为测试：批次形状、空批次短路、控制队列路径、队满丢弃的可观测计数、
//! 以及「释放 sink 才能真正关闭控制队列」的回归（`ControlLoop::close` 卡死的根因）。
//!
//! sink 是进程级槽、跨测试共享：只有 `releasing_the_sink_closes_the_control_queue` 动它
//! （先清空、结束时装回记录 sink），其余测试都用**自己的**记录 sink，互不依赖。

use std::sync::{Arc, Mutex};

use super::{LogBatch, LogSink, control_sink};
use crate::{ControlMessage, ControlWriter, LogLevel};

/// 测试用记录 sink（本地持有，不装到全局槽）。
fn recording_sink() -> (LogSink, Arc<Mutex<Vec<LogBatch>>>) {
    let records: Arc<Mutex<Vec<LogBatch>>> = Arc::default();
    let captured = Arc::clone(&records);
    let sink: LogSink = Arc::new(move |batch: LogBatch| {
        captured.lock().unwrap().push(batch);
    });
    (sink, records)
}

/// sink 收到的批次自带 scope / level / lines（AGENTS.md 的级别语义靠这三样表达）。
#[test]
fn deliveries_carry_scope_level_and_lines() {
    let (sink, records) = recording_sink();
    sink(LogBatch {
        scope: "window-controller".to_owned(),
        level: LogLevel::Warn,
        lines: vec!["refresh failed for /w".to_owned(), "second line".to_owned()],
    });
    let seen = records.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].scope, "window-controller");
    assert_eq!(seen[0].level, LogLevel::Warn);
    assert_eq!(seen[0].lines.len(), 2, "多行同批投递");
}

/// 空批次在进 sink 之前短路：不产生控制帧，也不计丢弃。
#[test]
fn empty_line_sets_are_not_delivered() {
    let (sink, records) = recording_sink();
    let dropped = super::dropped_batches();
    super::log_to(&sink, "logging-tests/empty", LogLevel::Error, Vec::new());
    assert_eq!(super::dropped_batches(), dropped, "空批次不算丢弃");
    assert!(records.lock().unwrap().is_empty(), "空批次不进 sink");
}

/// 控制队列路径：一条日志 = 一条 `LogBatch` 控制帧（不重不漏）。
#[test]
fn control_sink_routes_through_the_ordered_queue() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let sink = control_sink(writer);
    sink(LogBatch {
        scope: "logging-tests/queue".to_owned(),
        level: LogLevel::Info,
        lines: vec!["boot".to_owned()],
    });
    assert_eq!(
        rx.try_recv().expect("log batch enqueued"),
        ControlMessage::LogBatch {
            scope: "logging-tests/queue".to_owned(),
            level: LogLevel::Info,
            lines: vec!["boot".to_owned()],
        }
    );
    assert!(rx.try_recv().is_err(), "一条日志只入队一次");
}

/// 队列满即丢并计数（日志可丢，但丢弃要可观测）。
#[test]
fn full_queue_drops_are_counted() {
    let (writer, _rx) = ControlWriter::channel(1);
    let sink = control_sink(writer);
    let before = super::dropped_batches();
    sink(LogBatch {
        scope: "logging-tests/full".to_owned(),
        level: LogLevel::Info,
        lines: vec!["first".to_owned()],
    });
    sink(LogBatch {
        scope: "logging-tests/full".to_owned(),
        level: LogLevel::Warn,
        lines: vec!["second".to_owned()],
    });
    assert_eq!(super::dropped_batches(), before + 1, "第二条满队被丢弃");
}

/// 回归（收口卡死的根因）：sink 常持有控制队列的写句柄，只有 `release_sink` 把它从全局槽
/// 摘掉，队列才会关闭——否则 `ControlLoop::close` 等泵退出会一直挂着。
#[test]
fn releasing_the_sink_closes_the_control_queue() {
    super::release_sink(); // 本测试要自己拿写句柄，先清空全局槽
    let (writer, mut rx) = ControlWriter::channel(8);
    assert!(super::install_sink(control_sink(writer)), "槽已清空");
    super::info("logging-tests/release", "before release");
    assert_eq!(
        rx.try_recv().unwrap().kind(),
        "logBatch",
        "安装期间日志入队"
    );

    assert!(super::release_sink(), "释放持有写句柄的 sink");
    match rx.try_recv() {
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {}
        other => panic!("写句柄随 sink 释放 → 队列关闭，实际 {other:?}"),
    }
    assert!(!super::release_sink(), "释放幂等");
}
