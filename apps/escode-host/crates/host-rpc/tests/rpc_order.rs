//! 真实 `host-rpc` WebSocket 路径的保序（docs/specs/rust-u2-host-resilience.md §5.1，验收 H1.3）：
//!
//! - 同一连接：请求的**登记序 = 到达序**——处理函数的同步前缀按到达顺序执行（TS `ChannelServer`
//!   收到即同步 `channel.call`）；执行并发，响应按完成顺序发出、按 id 关联。
//! - 多连接并发：每个连接内部保序；跨连接不保证全局顺序。
//!
//! 处理函数的同步前缀把 `(conn, i)` 记入通道（观测登记序），异步部分等测试逐个放行
//! （观测响应关联）。全部由通道驱动，不用 sleep。
mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use escode_host_rpc::frame::FrameReader;
use escode_host_rpc::session::{self, Channel, Channels, handler};
use escode_host_rpc::wire::{Reader, Writer};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use support::*;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

type Gates = Arc<Mutex<BTreeMap<(i64, i64), oneshot::Receiver<()>>>>;

struct Probe {
    admitted: mpsc::UnboundedSender<(i64, i64)>,
    gates: Gates,
}

fn channels(probe: Probe) -> Channels {
    let Probe { admitted, gates } = probe;
    let order = Channel::new(
        "order",
        vec![(
            "record",
            handler(move |arg| {
                let value = arg.into_object().unwrap_or(Value::Null);
                let key = (
                    value["conn"].as_i64().unwrap_or(-1),
                    value["i"].as_i64().unwrap_or(-1),
                );
                // 同步前缀：登记序在这里被观测（读循环调用处理函数的那一刻）。
                let _ = admitted.send(key);
                let gate = gates.lock().unwrap().remove(&key);
                Box::pin(async move {
                    if let Some(gate) = gate {
                        let _ = gate.await;
                    }
                    Ok(Wire::Object(json!({"conn": key.0, "i": key.1})))
                })
            }),
        )],
    );
    Channels::new(vec![order])
}

/// 一个 Promise 请求帧（13 字节帧头 + VQL 头/体）。
fn promise_frame(id: i64, conn: i64, i: i64) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(session::REQUEST_PROMISE),
        Wire::Int(id),
        Wire::Str("order".to_owned()),
        Wire::Str("record".to_owned()),
    ]));
    writer.value(&Wire::Object(json!({"conn": conn, "i": i})));
    escode_host_rpc::frame::frame(&writer.into_bytes())
}

/// 把一个连接的 N 个请求帧拼进**一条** WebSocket 消息：读循环在一次读里背靠背地登记它们，
/// 处理函数若在独立任务里才被调用，多线程调度下登记序会与到达序不一致。
async fn send_batch(socket: &mut Socket, conn: i64, count: i64) {
    let mut batch = Vec::new();
    for i in 0..count {
        batch.extend(promise_frame(i, conn, i));
    }
    socket
        .send(Message::Binary(batch.into()))
        .await
        .expect("send batch");
}

/// 下一条 Promise 成功响应：(id, conn, i)。
async fn next_success(socket: &mut Socket, frames: &mut FrameReader) -> (i64, i64, i64) {
    loop {
        while let Some(body) = frames.next_frame().expect("frame") {
            let mut reader = Reader::new(&body);
            let Wire::Array(header) = reader.value().expect("header") else {
                panic!("header array")
            };
            let kind = header[0].as_int().expect("kind");
            if kind == session::RESPONSE_INITIALIZE {
                continue;
            }
            assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS, "{header:?}");
            let id = header[1].as_int().expect("id");
            let payload = reader
                .value()
                .expect("payload")
                .into_object()
                .expect("object payload");
            return (
                id,
                payload["conn"].as_i64().expect("conn"),
                payload["i"].as_i64().expect("i"),
            );
        }
        let message = tokio::time::timeout(Duration::from_secs(30), socket.next())
            .await
            .expect("frame within guard")
            .expect("open")
            .expect("frame ok");
        if let Message::Binary(bytes) = message {
            frames.push(&bytes);
        }
    }
}

async fn next_admitted(admitted: &mut mpsc::UnboundedReceiver<(i64, i64)>) -> (i64, i64) {
    tokio::time::timeout(Duration::from_secs(30), admitted.recv())
        .await
        .expect("admission within guard")
        .expect("probe open")
}

/// 为 `(conn, 0..count)` 建放行闸门，返回发送端（按 i 下标）。
fn gates_for(gates: &Gates, conn: i64, count: i64) -> Vec<oneshot::Sender<()>> {
    (0..count)
        .map(|i| {
            let (release, gate) = oneshot::channel();
            gates.lock().unwrap().insert((conn, i), gate);
            release
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_connection_admits_in_arrival_order_and_correlates_responses() {
    const N: i64 = 64;
    let (admitted_tx, mut admitted) = mpsc::unbounded_channel();
    let gates: Gates = Arc::default();
    let mut releases = gates_for(&gates, 0, N);
    let surface = escode_host_rpc::serve(
        "127.0.0.1:0".parse().unwrap(),
        channels(Probe {
            admitted: admitted_tx,
            gates: Arc::clone(&gates),
        }),
        None,
    )
    .await
    .expect("surface");
    let mut socket = open(&surface).await;
    let mut frames = FrameReader::new();

    send_batch(&mut socket, 0, N).await;
    let order: Vec<(i64, i64)> = {
        let mut order = Vec::new();
        for _ in 0..N {
            order.push(next_admitted(&mut admitted).await);
        }
        order
    };
    assert_eq!(
        order,
        (0..N).map(|i| (0, i)).collect::<Vec<_>>(),
        "登记序必须等于到达序"
    );

    // 倒序放行：响应按完成顺序到达，且每条都关联回自己的请求。
    for i in (0..N).rev() {
        releases
            .pop()
            .expect("release")
            .send(())
            .expect("handler waiting");
        assert_eq!(next_success(&mut socket, &mut frames).await, (i, 0, i));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_connections_each_keep_their_own_order() {
    const CONNECTIONS: i64 = 4;
    const N: i64 = 32;
    let (admitted_tx, mut admitted) = mpsc::unbounded_channel();
    let gates: Gates = Arc::default();
    let mut releases: BTreeMap<i64, Vec<oneshot::Sender<()>>> = (0..CONNECTIONS)
        .map(|conn| (conn, gates_for(&gates, conn, N)))
        .collect();
    let surface = escode_host_rpc::serve(
        "127.0.0.1:0".parse().unwrap(),
        channels(Probe {
            admitted: admitted_tx,
            gates: Arc::clone(&gates),
        }),
        None,
    )
    .await
    .expect("surface");
    let mut sockets = Vec::new();
    for _ in 0..CONNECTIONS {
        sockets.push(open(&surface).await);
    }

    // 各连接同时发出自己的整批请求。
    let sends = sockets
        .iter_mut()
        .enumerate()
        .map(|(conn, socket)| send_batch(socket, conn as i64, N));
    futures_util::future::join_all(sends).await;

    let mut per_connection: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for _ in 0..CONNECTIONS * N {
        let (conn, i) = next_admitted(&mut admitted).await;
        per_connection.entry(conn).or_default().push(i);
    }
    for conn in 0..CONNECTIONS {
        assert_eq!(
            per_connection.get(&conn),
            Some(&(0..N).collect::<Vec<_>>()),
            "连接 {conn} 内部登记序 = 到达序"
        );
    }

    // 每个连接倒序放行：响应只出现在自己的连接上，且关联回自己的请求。
    for (conn, socket) in sockets.iter_mut().enumerate() {
        let conn = conn as i64;
        let mut frames = FrameReader::new();
        let mut pending = releases.remove(&conn).expect("releases");
        for i in (0..N).rev() {
            pending
                .pop()
                .expect("release")
                .send(())
                .expect("handler waiting");
            assert_eq!(next_success(socket, &mut frames).await, (i, conn, i));
        }
    }
}
