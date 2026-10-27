use crate::domain::{MAX_REQUEST_BYTES, protocol::Request};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::io::Write;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use crate::contract::{Input, Output};
pub use crate::link::{EmbeddedIo, Link, MemoryLink, StdioLink, Streams};

/// 协议输入流集合：引擎 `serve` 的输入/输出与收口句柄（传输无关）。
/// 由 [`Link::start`] 产出；`stdio` 与内存双工链路共用同一套读写代码路径。
/// reader/writer 以工厂闭包传入：stdin/stdout 的锁不是 Send，必须在线程内创建与使用，
/// 因此 R/W 自身不要求 Send，只有工厂跨线程移动。
pub fn start_io<R, W, RF, WF>(
    make_reader: RF,
    make_writer: WF,
    cancel: CancellationToken,
    input_closed: CancellationToken,
) -> Streams
where
    R: std::io::BufRead + 'static,
    W: Write + 'static,
    RF: FnOnce() -> R + Send + 'static,
    WF: FnOnce() -> W + Send + 'static,
{
    let (in_tx, in_rx) = mpsc::channel(64);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<Value>>(64);
    super::stdio_input::spawn_reader(make_reader, in_tx, input_closed.clone(), dispatch);
    let progress = super::stdio_input::WriteProgress::default();
    let watch_progress = progress.clone();
    let watch_cancel = cancel.clone();
    escode_cli_core_api::scope::Spawner::current().spawn(
        "stdio-stall-watchdog",
        super::stdio_input::watch_stalled_output(watch_progress, input_closed, watch_cancel),
    );
    let writer_thread = std::thread::spawn(move || {
        let mut writer = make_writer();
        while let Some(batch) = out_rx.blocking_recv() {
            for message in batch {
                progress.begin();
                let result = (|| -> Result<()> {
                    let bytes = serde_json::to_vec(&message)?;
                    if bytes.len() + 1 > MAX_REQUEST_BYTES {
                        bail!("Protocol output exceeds frame limit");
                    }
                    writer.write_all(&bytes)?;
                    writer.write_all(b"\n")?;
                    writer.flush()?;
                    Ok(())
                })();
                progress.end();
                if result.is_err() {
                    cancel.cancel();
                    progress.finish();
                    return;
                }
            }
        }
        progress.finish();
    });
    Streams {
        input: in_rx,
        output: out_tx,
        writer: writer_thread,
    }
}
/// 需要按 Host 原键顺序重新解析 params 的方法（落盘时保持请求里的 key 顺序）。
const RAW_PARAMS_METHODS: &[&str] = &["plugins/configure"];

/// 类型化链路的读写线程（docs/specs/rust-u4-memory-link.md）：
/// 输入已是 `Value`（不再解析行），输出直接转发批量 `Value`（不再序列化）。
/// 通道容量、停滞看门狗与收口语义与 [`start_io`] 相同。
pub(crate) fn start_typed_io(
    mut inbound: mpsc::Receiver<Value>,
    outbound: mpsc::Sender<Vec<Value>>,
    cancel: CancellationToken,
    input_closed: CancellationToken,
) -> Streams {
    let (in_tx, in_rx) = mpsc::channel(64);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<Value>>(64);
    let forward_cancel = input_closed.clone();
    std::thread::spawn(move || {
        while let Some(value) = inbound.blocking_recv() {
            if frame_exceeds_limit(&value) {
                let _ = in_tx.blocking_send(Input::TooLarge);
                forward_cancel.cancel();
                return;
            }
            if !dispatch_value(&in_tx, value) {
                return;
            }
        }
        let _ = in_tx.blocking_send(Input::Eof);
        forward_cancel.cancel();
    });
    let progress = super::stdio_input::WriteProgress::default();
    let watch_progress = progress.clone();
    let watch_cancel = cancel.clone();
    escode_cli_core_api::scope::Spawner::current().spawn(
        "stdio-stall-watchdog",
        super::stdio_input::watch_stalled_output(watch_progress, input_closed, watch_cancel),
    );
    let writer_thread = std::thread::spawn(move || {
        while let Some(batch) = out_rx.blocking_recv() {
            progress.begin();
            let result = outbound.blocking_send(batch);
            progress.end();
            if result.is_err() {
                cancel.cancel();
                progress.finish();
                return;
            }
        }
        progress.finish();
    });
    Streams {
        input: in_rx,
        output: out_tx,
        writer: writer_thread,
    }
}

/// 类型化帧的 `MAX_REQUEST_BYTES` 判定：与字节链路结果一致，常态零序列化。
///
/// - 最小编码下界 > 限额 → 拒绝（不必精确测量）；
/// - 最大编码上界 ≤ 限额 → 接受；
/// - 否则序列化一次取精确长度（只有贴近限额的帧付出这次成本）。
fn frame_exceeds_limit(value: &Value) -> bool {
    let (min, max) = json_size_bounds(value);
    if min > MAX_REQUEST_BYTES {
        return true;
    }
    if max <= MAX_REQUEST_BYTES {
        return false;
    }
    match serde_json::to_vec(value) {
        // 字节链路把结尾换行也计入限额，这里保持同一口径。
        Ok(bytes) => bytes.len() + 1 > MAX_REQUEST_BYTES,
        Err(_) => true,
    }
}

/// JSON 编码长度下界/上界的保守估计（同一遍递归）。
fn json_size_bounds(value: &Value) -> (usize, usize) {
    match value {
        Value::Null => (4, 4),
        Value::Bool(true) => (4, 4),
        Value::Bool(false) => (5, 5),
        // 数字至少 1 字节；f64 十进制展开远小于 32。
        Value::Number(_) => (1, 32),
        Value::String(text) => (
            text.len().saturating_add(2),
            text.len().saturating_mul(6).saturating_add(2),
        ),
        Value::Array(items) => {
            let mut min = 2usize;
            let mut max = 2usize;
            for item in items {
                let (item_min, item_max) = json_size_bounds(item);
                min = min.saturating_add(item_min).saturating_add(1);
                max = max.saturating_add(item_max).saturating_add(1);
            }
            (min, max)
        }
        Value::Object(map) => {
            let mut min = 2usize;
            let mut max = 2usize;
            for (key, item) in map {
                let (item_min, item_max) = json_size_bounds(item);
                // key 编码 + 引号 + 冒号。
                min = min
                    .saturating_add(key.len())
                    .saturating_add(item_min)
                    .saturating_add(4);
                max = max
                    .saturating_add(key.len().saturating_mul(6))
                    .saturating_add(item_max)
                    .saturating_add(4);
            }
            (min, max)
        }
    }
}

fn dispatch(tx: &mpsc::Sender<Input>, line: &[u8]) -> bool {
    if line.iter().all(u8::is_ascii_whitespace) {
        return true;
    }
    match serde_json::from_slice::<Value>(line) {
        Ok(value) => dispatch_frame(tx, value, Some(line)),
        Err(_) => tx.blocking_send(Input::Invalid).is_ok(),
    }
}

/// 类型化链路的唯一解释器（stage 4）：与 [`dispatch`] 共用同一段语义。
/// `line` 是字节链路的原始行，只为保留 Host 原键顺序（`raw_result`/`raw_params`）；类型化链路
/// 传 None，用 `Value::to_string()` 重编码——本工作区未启用 `preserve_order`，键序与字节链路一致。
pub(crate) fn dispatch_value(tx: &mpsc::Sender<Input>, value: Value) -> bool {
    if value.is_null() {
        return tx.blocking_send(Input::Invalid).is_ok();
    }
    dispatch_frame(tx, value, None)
}

fn dispatch_frame(tx: &mpsc::Sender<Input>, value: Value, line: Option<&[u8]>) -> bool {
    if value.get("method").is_none()
        && let Some(id) = value["id"].as_str()
        && (value.get("result").is_some() ^ value.get("error").is_some())
    {
        // 原始 result 文本：部分工具（Cron）需要按 Host 原键顺序重新解析。
        let raw_result = match line {
            Some(line) => serde_json::from_slice::<
                std::collections::HashMap<String, Box<serde_json::value::RawValue>>,
            >(line)
            .ok()
            .and_then(|reply| reply.get("result").map(|raw| raw.get().to_owned())),
            None => value.get("result").map(Value::to_string),
        };
        return tx
            .blocking_send(Input::Response {
                id: id.into(),
                result: value.get("result").cloned().unwrap_or(Value::Null),
                error: value.get("error").cloned(),
                raw_result,
            })
            .is_ok();
    }
    // `v4/command` 的 schema 校验（rust-unified-host.md P1 #10）：语料由 TS zod 生成，
    // 违规在此记录为 debug 日志（协议入口是唯一能看见原始 params 的 adapter 边界）。
    // 不改变应答：现有 serde 拒绝面与 TS zod 不完全一致，收紧线上行为需差分场景作证。
    if value["method"] == "v4/command" {
        let violations = escode_cli_protocol::command_schema::validate_command(&value["params"]);
        if !violations.is_empty() {
            escode_cli_host::file_log::log(
                escode_cli_host::file_log::Level::Debug,
                "command.schema.violations",
                "cli",
                &escode_cli_protocol::command_schema::describe(&violations),
                None,
                json!({"commandId": value["params"]["commandId"]}),
            );
        }
    }
    let request = {
        if value["method"] == "startup/storagePathReady" {
            serde_json::from_value(
                json!({"method":"startup/storagePathReady","params":{"reuse":value["reuse"]}}),
            )
        } else {
            let method = value["method"].as_str().unwrap_or_default().to_owned();
            let params = value.get("params").cloned();
            serde_json::from_value::<Request>(value).map(|mut request| {
                if RAW_PARAMS_METHODS.contains(&method.as_str()) {
                    request.raw_params = Some(match line {
                        Some(line) => serde_json::from_slice::<
                            std::collections::HashMap<String, Box<serde_json::value::RawValue>>,
                        >(line)
                        .ok()
                        .and_then(|fields| fields.get("params").map(|raw| raw.get().to_owned())),
                        None => params.map(|params| params.to_string()),
                    })
                    .flatten();
                }
                request
            })
        }
    };
    tx.blocking_send(match request {
        Ok(r) if !r.method.trim().is_empty() => Input::Request(r),
        _ => Input::Invalid,
    })
    .is_ok()
}
pub async fn finish(writer: std::thread::JoinHandle<()>) -> Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !writer.is_finished() {
        if tokio::time::Instant::now() >= deadline {
            bail!("Protocol output drain timed out");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("Protocol writer failed"))
}

pub async fn storage_prepare(
    path: &std::path::Path,
    input: &mut mpsc::Receiver<Input>,
    output: &Output,
) -> Result<()> {
    output
        .send(vec![
            json!({"method":"startup/storagePath","params":{"path":path}}),
        ])
        .await?;
    match tokio::time::timeout(std::time::Duration::from_secs(30), input.recv()).await? {
        Some(Input::Request(request)) if request.method == "startup/storagePathReady" => Ok(()),
        _ => bail!("Storage preparation handshake failed"),
    }
}
