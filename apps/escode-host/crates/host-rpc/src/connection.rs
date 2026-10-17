//! 一条 RPC 连接的事件循环（docs/specs/rust-u2-agent-v4.md §3.1）。
//!
//! ```text
//! 读循环 ──Promise（同步登记：调用处理函数）──► 独立任务 ──响应──► 出站队列 ──► 写任务 ──► WebSocket
//!        ──EventListen──► 事件转发任务 ──EventFire──┘
//!        ──PromiseCancel / EventDispose──► 中止对应任务
//! 关闭：中止全部任务 → 水位置 closed → 依次执行连接级 channel 的关闭钩子
//! ```
//!
//! 修复（评审 A3）：旧实现在读循环里 `respond(..).await`，一个慢请求阻塞同连接全部请求、
//! `PromiseCancel` 永远无效。TS `ChannelServer` 每条请求独立 Promise，这里同构。
//! 保序（docs/specs/rust-u2-host-resilience.md §5.1）：每连接的登记序 = 到达序——处理函数在读循环内
//! 按到达顺序被调用（同步前缀此刻执行），异步部分并发执行，响应按完成顺序发出、按 id 关联。
//!
//! 出站队列溢出策略：不丢帧。字节水位高 8 MiB → `saturated`、低 2 MiB → `drained`
//! （边沿经 [`ConnectionContext::flow`] 交给连接级 channel，V4 面转发为 `v4/connection/flow`，
//! runtime 暂停投递、恢复时补快照）；硬上限 64 MiB → 关闭连接（fail closed）。

use escode_host_control::logging;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::session::{self, Channels, ClientMode, ConnectionContext, FlowState, Request};

// 字节水位与硬上限：`rust-u2-host-skeleton.md` §3.2「RPC 连接出站」行（不丢帧，超硬上限关闭连接）。
const HIGH_WATER: usize = 8 * 1024 * 1024;
const LOW_WATER: usize = 2 * 1024 * 1024;
const HARD_LIMIT: usize = 64 * 1024 * 1024;

/// 出站队列：字节计数 + 水位边沿 + 硬上限关闭信号。
#[derive(Clone)]
struct Outbound {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    flow: Arc<watch::Sender<FlowState>>,
    overflow: Arc<Notify>,
}

impl Outbound {
    fn push(&self, body: Vec<u8>) {
        if body.is_empty() {
            return;
        }
        let framed = crate::frame::frame(&body);
        let total = self.queued.fetch_add(framed.len(), Ordering::SeqCst) + framed.len();
        if self.tx.send(framed).is_err() {
            return;
        }
        if total > HARD_LIMIT {
            self.overflow.notify_one();
        } else if total > HIGH_WATER {
            self.flow.send_if_modified(|state| {
                let edge = *state == FlowState::Drained;
                if edge {
                    *state = FlowState::Saturated;
                }
                edge
            });
        }
    }

    fn sent(&self, bytes: usize) {
        let total = self.queued.fetch_sub(bytes, Ordering::SeqCst) - bytes;
        if total < LOW_WATER {
            self.flow.send_if_modified(|state| {
                let edge = *state == FlowState::Saturated;
                if edge {
                    *state = FlowState::Drained;
                }
                edge
            });
        }
    }
}

/// 在飞任务表（Promise 与事件订阅共用 id 空间，与 TS `activeRequests` 同）。
#[derive(Default)]
struct Active(Mutex<BTreeMap<i64, JoinHandle<()>>>);

impl Active {
    fn insert(&self, id: i64, task: JoinHandle<()>) {
        let mut tasks = self.lock();
        // 已结束的任务顺手摘除（任务结束时不回写本表，避免与登记竞态）。
        tasks.retain(|_, task| !task.is_finished());
        if let Some(previous) = tasks.insert(id, task) {
            previous.abort();
        }
    }

    fn abort(&self, id: i64) {
        if let Some(task) = self.lock().remove(&id) {
            task.abort();
        }
    }

    fn abort_all(&self) {
        for (_, task) in std::mem::take(&mut *self.lock()) {
            task.abort();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<i64, JoinHandle<()>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 运行一条 RPC 连接直到对端关闭。
pub(crate) async fn run(
    ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    channels: Arc<Channels>,
    client_mode: ClientMode,
) -> Result<()> {
    let (mut sink, mut source) = ws.split();
    let (flow_tx, flow_rx) = watch::channel(FlowState::Drained);
    let context = ConnectionContext::new(client_mode, flow_rx);
    let channels = Arc::new(channels.connect(&context));
    for hook in channels.open_hooks() {
        hook();
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let outbound = Outbound {
        tx,
        queued: Arc::default(),
        flow: Arc::new(flow_tx),
        overflow: Arc::default(),
    };
    // 连接建立即 Initialize（TS ChannelServer 构造时发送）。
    outbound.push(session::initialize_frame());
    let writer_outbound = outbound.clone();
    let writer = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            let len = bytes.len();
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
            writer_outbound.sent(len);
        }
        let _ = sink.close().await;
    });
    let active = Arc::new(Active::default());
    let mut frames = crate::frame::FrameReader::new();
    let result: Result<()> = async {
        loop {
            let message = tokio::select! {
                message = source.next() => message,
                () = outbound.overflow.notified() => {
                    anyhow::bail!("outbound backlog exceeded {HARD_LIMIT} bytes; closing connection")
                }
            };
            match message {
                Some(Ok(Message::Binary(bytes))) => frames.push(&bytes),
                Some(Ok(Message::Text(text))) => frames.push(text.as_bytes()),
                Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
                Some(Ok(_)) => continue,
            }
            while let Some(body) = frames.next_frame().context("invalid RPC frame stream")? {
                match session::parse_request(&body) {
                    Ok(request) => dispatch(&channels, &outbound, &active, request),
                    Err(error) => logging::warn("rpc", format!("invalid RPC frame: {error:#}")),
                }
            }
        }
    }
    .await;
    // 收口：先中止在飞请求与事件转发，再置 closed，最后跑连接级清理钩子（V4 注销订阅）。
    active.abort_all();
    outbound.flow.send_replace(FlowState::Closed);
    for hook in channels.close_hooks() {
        hook().await;
    }
    drop(outbound);
    let _ = writer.await;
    result
}

fn dispatch(channels: &Arc<Channels>, outbound: &Outbound, active: &Arc<Active>, request: Request) {
    match request.kind {
        session::REQUEST_EVENT_LISTEN => listen(channels, outbound, active, request),
        session::REQUEST_PROMISE_CANCEL | session::REQUEST_EVENT_DISPOSE => {
            active.abort(request.id)
        }
        // Promise 与未知类型（回 InvalidRequest 错误帧）：在读循环内同步登记（处理函数此刻被调用，
        // 登记序 = 到达序），执行与应答在独立任务里（docs/specs/rust-u2-host-resilience.md §5.1）。
        _ => {
            let responding = session::start(channels, &request);
            let outbound = outbound.clone();
            let task = tokio::spawn(async move {
                outbound.push(responding.await);
            });
            active.insert(request.id, task);
        }
    }
}

fn listen(channels: &Arc<Channels>, outbound: &Outbound, active: &Arc<Active>, request: Request) {
    let Some(handler) = channels
        .channel(&request.channel)
        .and_then(|channel| channel.event(&request.method))
    else {
        // TS：未知 channel 的事件请求挂起等待注册、超时后也不回帧；未知事件在服务端抛错。
        // 两种情况客户端都收不到响应，这里同样只记日志。
        logging::warn(
            "rpc",
            format!(
                "event `{}.{}` is not implemented",
                request.channel, request.method
            ),
        );
        return;
    };
    let mut stream = match handler(request.arg) {
        Ok(stream) => stream,
        Err(error) => {
            logging::warn(
                "rpc",
                format!(
                    "event `{}.{}` listen failed: {error}",
                    request.channel, request.method
                ),
            );
            return;
        }
    };
    let outbound = outbound.clone();
    let id = request.id;
    let task = tokio::spawn(async move {
        while let Some(value) = stream.next().await {
            outbound.push(session::event_frame(id, &value));
        }
    });
    active.insert(id, task);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn outbound() -> (
        Outbound,
        mpsc::UnboundedReceiver<Vec<u8>>,
        watch::Receiver<FlowState>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (flow, flow_rx) = watch::channel(FlowState::Drained);
        (
            Outbound {
                tx,
                queued: Arc::default(),
                flow: Arc::new(flow),
                overflow: Arc::default(),
            },
            rx,
            flow_rx,
        )
    }

    /// 出站队列越过 8 MiB → saturated；写回后低于 2 MiB → drained（边沿只发一次）。
    #[tokio::test]
    async fn outbound_watermarks_emit_saturated_and_drained_edges() {
        let (outbound, mut rx, mut flow) = outbound();
        outbound.push(vec![0; HIGH_WATER + 1]);
        assert!(
            rx.recv().await.is_some(),
            "overflowing frames are never dropped"
        );
        flow.changed().await.expect("saturated edge");
        assert_eq!(*flow.borrow_and_update(), FlowState::Saturated);

        let queued = outbound.queued.load(Ordering::SeqCst);
        assert!(queued > HIGH_WATER, "queued bytes: {queued}");
        outbound.sent(queued);
        flow.changed().await.expect("drained edge");
        assert_eq!(*flow.borrow_and_update(), FlowState::Drained);
    }

    /// 超过 64 MiB 硬上限：通知连接循环关闭连接（fail closed），不是丢弃帧。
    #[tokio::test]
    async fn outbound_hard_limit_signals_connection_close() {
        let (outbound, _rx, _flow) = outbound();
        outbound.push(vec![0; HARD_LIMIT + 1]);
        tokio::time::timeout(Duration::from_secs(1), outbound.overflow.notified())
            .await
            .expect("hard-limit overflow must wake the connection loop");
    }
}
