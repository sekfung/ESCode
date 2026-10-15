//! `window-controller` 端到端测试的连接客户端（真 WebSocket + controller 帧收集）。
//! 从 `tests/rpc_window_controller.rs` 拆出以守住单文件行数上限。

use std::time::Duration;

use serde_json::Value;

pub use crate::support::*;

pub struct ControllerClient {
    pub socket: Socket,
    frames: FrameReader,
    next: i64,
    /// 本连接收到的 controller 帧（`onDynamicControllerFrame`）。
    pub controller: Vec<Value>,
    pub listen_id: i64,
}

impl ControllerClient {
    /// 真实调用方路径：申请一次性 capability 后连 `/ws/host`。
    pub async fn new_client(surface: &crate::support::RpcSurface) -> Self {
        Self::new(crate::support::open(surface).await)
    }

    pub fn new(socket: Socket) -> Self {
        Self {
            socket,
            frames: FrameReader::new(),
            next: 0,
            controller: Vec::new(),
            listen_id: 0,
        }
    }

    pub async fn send(&mut self, kind: i64, channel: &str, name: &str, arg: Wire) -> i64 {
        self.next += 1;
        let id = self.next;
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str(channel.to_owned()),
            Wire::Str(name.to_owned()),
        ]));
        writer.value(&arg);
        self.socket
            .send(Message::Binary(
                escode_host_rpc::frame::frame(&writer.into_bytes()).into(),
            ))
            .await
            .unwrap();
        id
    }

    async fn frame(&mut self) -> (i64, i64, Wire) {
        loop {
            while let Some(body) = self.frames.next_frame().unwrap() {
                let mut reader = Reader::new(&body);
                let Wire::Array(header) = reader.value().unwrap() else {
                    panic!("header")
                };
                let kind = header[0].as_int().unwrap();
                if kind == session::RESPONSE_INITIALIZE {
                    continue;
                }
                return (kind, header[1].as_int().unwrap(), reader.value().unwrap());
            }
            let message = tokio::time::timeout(Duration::from_secs(30), self.socket.next())
                .await
                .expect("frame in time")
                .unwrap()
                .unwrap();
            if let Message::Binary(bytes) = message {
                self.frames.push(&bytes);
            }
        }
    }

    /// 调用并等应答；期间到达的 controller 帧进 `controller`。
    pub async fn call(&mut self, channel: &str, name: &str, params: Value) -> Result<Wire, Value> {
        let id = self
            .send(
                session::REQUEST_PROMISE,
                channel,
                name,
                Wire::Array(vec![Wire::Object(params)]),
            )
            .await;
        loop {
            let (kind, response, data) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE && response == self.listen_id {
                if let Wire::Object(frame) = data {
                    self.controller.push(frame);
                }
                continue;
            }
            if response != id {
                continue;
            }
            return if kind == session::RESPONSE_PROMISE_SUCCESS {
                Ok(data)
            } else {
                Err(data.into_object().unwrap_or(Value::Null))
            };
        }
    }

    /// 等一帧满足谓词的 controller 帧（帧与应答的相对顺序不保证）。
    pub async fn controller_frame(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        loop {
            if let Some(position) = self.controller.iter().position(&predicate) {
                return self.controller.remove(position);
            }
            let (kind, response, data) = self.frame().await;
            if kind == session::RESPONSE_EVENT_FIRE
                && response == self.listen_id
                && let Wire::Object(frame) = data
            {
                self.controller.push(frame);
            }
        }
    }
}
