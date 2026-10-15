//! RPC 连接层（docs/specs/rust-u2-agent-v4.md §3.1 / §4-1，评审 A3）：并发请求、真取消、
//! `EventListen` → `EventFire` → `EventDispose`、连接级 channel 与关闭钩子。
//!
//! 用自定义 channel 驱动，时序全部由测试控制（不依赖 runtime）。
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use escode_host_rpc::session::{Channel, Channels, ClientMode, handler};
use support::*;
use tokio::sync::Notify;

struct Probe {
    release_slow: Arc<Notify>,
    slow_finished: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    connection_ids: Arc<std::sync::Mutex<Vec<(String, ClientMode)>>>,
}

fn channels(probe: &Probe) -> Channels {
    let release = Arc::clone(&probe.release_slow);
    let finished = Arc::clone(&probe.slow_finished);
    let test = Channel::new(
        "test",
        vec![
            (
                "slow",
                handler(move |_arg| {
                    let release = Arc::clone(&release);
                    let finished = Arc::clone(&finished);
                    Box::pin(async move {
                        release.notified().await;
                        finished.fetch_add(1, Ordering::SeqCst);
                        Ok(Wire::Str("slow".to_owned()))
                    })
                }),
            ),
            (
                "fast",
                handler(|_arg| Box::pin(async { Ok(Wire::Str("fast".to_owned())) })),
            ),
        ],
    )
    .with_events(vec![(
        "onDynamicTick",
        Arc::new(|arg: Wire| {
            // 动态事件的实参决定事件流内容（TS `onDynamicX(arg)` 同形）。
            let prefix = arg
                .into_object()
                .and_then(|value| value["prefix"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let stream = futures_util::stream::unfold(0_i64, move |n| {
                let prefix = prefix.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Some((Wire::Str(format!("{prefix}{n}")), n + 1))
                }
            });
            Ok(Box::pin(stream) as escode_host_rpc::session::EventStream)
        }),
    )]);
    let closed = Arc::clone(&probe.closed);
    let ids = Arc::clone(&probe.connection_ids);
    Channels::new(vec![test]).with_scoped(Arc::new(move |context| {
        ids.lock()
            .unwrap()
            .push((context.connection_id.clone(), context.client_mode));
        let closed = Arc::clone(&closed);
        Channel::new("scoped", vec![]).with_close(Arc::new(move || {
            let closed = Arc::clone(&closed);
            Box::pin(async move {
                closed.fetch_add(1, Ordering::SeqCst);
            })
        }))
    }))
}

async fn surface(probe: &Probe) -> RpcSurface {
    escode_host_rpc::serve("127.0.0.1:0".parse().unwrap(), channels(probe), None)
        .await
        .expect("surface")
}

fn probe() -> Probe {
    Probe {
        release_slow: Arc::default(),
        slow_finished: Arc::default(),
        closed: Arc::default(),
        connection_ids: Arc::default(),
    }
}

async fn send(socket: &mut Socket, kind: i64, id: i64, channel: &str, name: &str, arg: &Wire) {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(kind),
        Wire::Int(id),
        Wire::Str(channel.to_owned()),
        Wire::Str(name.to_owned()),
    ]));
    writer.value(arg);
    socket
        .send(Message::Binary(
            escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
        ))
        .await
        .expect("send");
}

/// 读下一条非 Initialize 响应：(类型, id, 数据)。
async fn next_response(socket: &mut Socket, frames: &mut FrameReader) -> (i64, i64, Wire) {
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
            let id = header[1].as_int().expect("id");
            return (kind, id, reader.value().expect("payload"));
        }
        let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("frame within timeout")
            .expect("open")
            .expect("frame ok");
        if let Message::Binary(bytes) = message {
            frames.push(&bytes);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_request_does_not_block_the_connection_and_cancel_aborts_it() {
    let probe = probe();
    let surface = surface(&probe).await;
    let mut socket = open(&surface).await;
    let mut frames = FrameReader::new();

    // 慢请求在前、快请求在后：快请求先回（旧实现串行，快请求会被挡住）。
    send(
        &mut socket,
        session::REQUEST_PROMISE,
        1,
        "test",
        "slow",
        &Wire::Undefined,
    )
    .await;
    send(
        &mut socket,
        session::REQUEST_PROMISE,
        2,
        "test",
        "fast",
        &Wire::Undefined,
    )
    .await;
    let (kind, id, data) = next_response(&mut socket, &mut frames).await;
    assert_eq!((kind, id), (session::RESPONSE_PROMISE_SUCCESS, 2));
    assert_eq!(data, Wire::Str("fast".to_owned()));

    // 取消在飞的慢请求：放行后它也不会完成、不会回帧。
    send(
        &mut socket,
        session::REQUEST_PROMISE_CANCEL,
        1,
        "",
        "",
        &Wire::Undefined,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    probe.release_slow.notify_waiters();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        probe.slow_finished.load(Ordering::SeqCst),
        0,
        "cancelled task ran to completion"
    );

    // 未取消的慢请求正常完成。
    send(
        &mut socket,
        session::REQUEST_PROMISE,
        3,
        "test",
        "slow",
        &Wire::Undefined,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    probe.release_slow.notify_waiters();
    let (kind, id, data) = next_response(&mut socket, &mut frames).await;
    assert_eq!((kind, id), (session::RESPONSE_PROMISE_SUCCESS, 3));
    assert_eq!(data, Wire::Str("slow".to_owned()));
    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dynamic_events_fire_until_disposed() {
    let probe = probe();
    let surface = surface(&probe).await;
    let mut socket = open(&surface).await;
    let mut frames = FrameReader::new();

    let arg = Wire::Object(serde_json::json!({"prefix": "t"}));
    send(
        &mut socket,
        session::REQUEST_EVENT_LISTEN,
        5,
        "test",
        "onDynamicTick",
        &arg,
    )
    .await;
    for expected in ["t0", "t1", "t2"] {
        let (kind, id, data) = next_response(&mut socket, &mut frames).await;
        assert_eq!((kind, id), (session::RESPONSE_EVENT_FIRE, 5));
        assert_eq!(data, Wire::Str(expected.to_owned()));
    }
    send(
        &mut socket,
        session::REQUEST_EVENT_DISPOSE,
        5,
        "",
        "",
        &Wire::Undefined,
    )
    .await;
    // 注销后不再有 EventFire：随后的 Promise 应答是下一条且只有它。
    tokio::time::sleep(Duration::from_millis(100)).await;
    send(
        &mut socket,
        session::REQUEST_PROMISE,
        6,
        "test",
        "fast",
        &Wire::Undefined,
    )
    .await;
    loop {
        let (kind, id, _) = next_response(&mut socket, &mut frames).await;
        if id == 6 {
            assert_eq!(kind, session::RESPONSE_PROMISE_SUCCESS);
            break;
        }
        // 注销请求与最后一次 fire 之间可能还有在途帧；注销生效后不得再出现。
        assert_eq!(kind, session::RESPONSE_EVENT_FIRE);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    send(
        &mut socket,
        session::REQUEST_PROMISE,
        7,
        "test",
        "fast",
        &Wire::Undefined,
    )
    .await;
    let (_, id, _) = next_response(&mut socket, &mut frames).await;
    assert_eq!(id, 7, "event kept firing after EventDispose");
    surface.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scoped_channels_are_per_connection_and_close_hooks_run_on_disconnect() {
    let probe = probe();
    let surface = surface(&probe).await;
    let host = open(&surface).await;
    let capability = issue_capability(&surface).await;
    let replayable = connect(surface.addr, "/ws", Some(&capability)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    {
        let ids = probe.connection_ids.lock().unwrap();
        // 探测方法表的那一次 + 两条连接各一次。
        let connections: Vec<_> = ids.iter().skip(ids.len() - 2).cloned().collect();
        assert_eq!(connections[0].1, ClientMode::DesktopContinuous);
        assert_eq!(connections[1].1, ClientMode::WebRemoteReplayable);
        assert_ne!(connections[0].0, connections[1].0);
        assert!(
            connections
                .iter()
                .all(|(id, _)| id.starts_with("host-rpc-"))
        );
    }
    assert_eq!(probe.closed.load(Ordering::SeqCst), 0);
    drop(host);
    drop(replayable);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while probe.closed.load(Ordering::SeqCst) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "close hooks did not run"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    surface.shutdown().await;
}
