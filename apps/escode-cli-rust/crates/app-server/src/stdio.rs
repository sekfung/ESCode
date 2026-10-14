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

fn dispatch(tx: &mpsc::Sender<Input>, line: &[u8]) -> bool {
    if line.iter().all(u8::is_ascii_whitespace) {
        return true;
    }
    let value = match serde_json::from_slice::<Value>(line) {
        Ok(v) => v,
        Err(_) => return tx.blocking_send(Input::Invalid).is_ok(),
    };
    if value.get("method").is_none()
        && let Some(id) = value["id"].as_str()
        && (value.get("result").is_some() ^ value.get("error").is_some())
    {
        // 原始 result 文本：部分工具（Cron）需要按 Host 原键顺序重新解析。
        let raw_result = serde_json::from_slice::<
            std::collections::HashMap<String, Box<serde_json::value::RawValue>>,
        >(line)
        .ok()
        .and_then(|reply| reply.get("result").map(|raw| raw.get().to_owned()));
        return tx
            .blocking_send(Input::Response {
                id: id.into(),
                result: value.get("result").cloned().unwrap_or(Value::Null),
                error: value.get("error").cloned(),
                raw_result,
            })
            .is_ok();
    }
    let request = {
        if value["method"] == "startup/storagePathReady" {
            serde_json::from_value(
                json!({"method":"startup/storagePathReady","params":{"reuse":value["reuse"]}}),
            )
        } else {
            let method = value["method"].as_str().unwrap_or_default().to_owned();
            serde_json::from_value::<Request>(value).map(|mut request| {
                if RAW_PARAMS_METHODS.contains(&method.as_str()) {
                    request.raw_params = serde_json::from_slice::<
                        std::collections::HashMap<String, Box<serde_json::value::RawValue>>,
                    >(line)
                    .ok()
                    .and_then(|fields| fields.get("params").map(|raw| raw.get().to_owned()));
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
