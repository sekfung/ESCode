//! RPC 会话：`ChannelServer`/`ChannelClient` 的 Rust 侧对端（packages/rpc/src/channelServer.ts）。
//!
//! 连接建立即发 `Initialize`（`[200]` + Undefined）；随后按请求类型处理：
//!
//! | 请求（RequestType） | 行为 |
//! | --- | --- |
//! | `Promise`(100) | 查 channel 表；命中即执行并回 `[201, id]` + 数据；失败回 `[202, id]` + 结构化错误 |
//! | `PromiseCancel`(101) / `EventDispose`(103) | 释放该 id（当前无长任务，等价 no-op） |
//! | `EventListen`(102) | 回结构化错误（事件面尚未迁移，缺口显式） |
//!
//! 未迁移的 channel 回 `NotImplemented` 错误（Renderer 见显式缺口，不挂起）。
//! channel 名来自 TS `ServiceChannels`（`packages/shared/src/channels.ts`）；方法名在请求头第 4 项。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::json;

use crate::wire::{Reader, Wire, Writer};

/// 请求类型（TS `RequestType`）。
pub const REQUEST_PROMISE: i64 = 100;
pub const REQUEST_PROMISE_CANCEL: i64 = 101;
pub const REQUEST_EVENT_LISTEN: i64 = 102;
pub const REQUEST_EVENT_DISPOSE: i64 = 103;

/// 响应类型（TS `ResponseType`）。
pub const RESPONSE_INITIALIZE: i64 = 200;
pub const RESPONSE_PROMISE_SUCCESS: i64 = 201;
pub const RESPONSE_PROMISE_ERROR: i64 = 202;

/// 一条已解析的请求。
#[derive(Debug, PartialEq)]
pub struct Request {
    pub kind: i64,
    pub id: i64,
    pub channel: String,
    pub method: String,
    pub arg: Wire,
}

/// 编码 `Initialize` 响应（连接建立后服务端的第一帧）。
pub fn initialize_frame() -> Vec<u8> {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![Wire::Int(RESPONSE_INITIALIZE)]));
    writer.value(&Wire::Undefined);
    writer.into_bytes()
}

/// 编码 `PromiseSuccess`。
pub fn success_frame(id: i64, data: &Wire) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(RESPONSE_PROMISE_SUCCESS),
        Wire::Int(id),
    ]));
    writer.value(data);
    writer.into_bytes()
}

/// 编码 `PromiseError`（TS 的 `{message, name, stack}` 形状）。
pub fn error_frame(id: i64, name: &str, message: &str) -> Vec<u8> {
    let payload = json!({
        "message": message,
        "name": name,
        "stack": [message],
    });
    let mut writer = Writer::new();
    writer.value(&Wire::Array(vec![
        Wire::Int(RESPONSE_PROMISE_ERROR),
        Wire::Int(id),
    ]));
    writer.value(&Wire::Object(payload));
    writer.into_bytes()
}

/// 解析一条请求帧（`serialize([type, id, channel, method])` + `serialize(arg)`）。
pub fn parse_request(body: &[u8]) -> anyhow::Result<Request> {
    let mut reader = Reader::new(body);
    let header = reader.value()?;
    let arg = reader.value()?;
    let Wire::Array(items) = header else {
        anyhow::bail!("RPC request header must be an array");
    };
    anyhow::ensure!(items.len() >= 2, "RPC request header is too short");
    let kind = items[0]
        .as_int()
        .ok_or_else(|| anyhow::anyhow!("RPC request type must be an integer"))?;
    let id = items[1]
        .as_int()
        .ok_or_else(|| anyhow::anyhow!("RPC request id must be an integer"))?;
    Ok(Request {
        kind,
        id,
        channel: items
            .get(2)
            .and_then(Wire::as_str)
            .unwrap_or_default()
            .to_owned(),
        method: items
            .get(3)
            .and_then(Wire::as_str)
            .unwrap_or_default()
            .to_owned(),
        arg,
    })
}

/// 处理结果：成功数据，或 `(错误名, 消息)`。
pub type ChannelResult = Result<Wire, (String, String)>;
/// channel 处理函数（方法名 + 入参 → 结果）。
pub type ChannelHandler =
    Arc<dyn Fn(String, Wire) -> Pin<Box<dyn Future<Output = ChannelResult> + Send>> + Send + Sync>;

/// channel 表：已迁移的服务域登记在这里；未登记即显式缺口。
#[derive(Default)]
pub struct Channels {
    entries: Vec<(&'static str, ChannelHandler)>,
}

impl Channels {
    pub fn new(entries: Vec<(&'static str, ChannelHandler)>) -> Self {
        Self { entries }
    }

    pub fn handler(&self, channel: &str) -> Option<ChannelHandler> {
        self.entries
            .iter()
            .find(|(name, _)| *name == channel)
            .map(|(_, handler)| Arc::clone(handler))
    }

    /// 已登记的 channel 名（能力清单与诊断用）。
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|(name, _)| *name).collect()
    }
}

/// 处理一条请求，返回要回给客户端的响应 body（可能为空：取消类请求无响应）。
pub async fn respond(channels: &Channels, request: &Request) -> Vec<u8> {
    match request.kind {
        REQUEST_PROMISE => match channels.handler(&request.channel) {
            Some(handler) => match handler(request.method.clone(), request.arg.clone()).await {
                Ok(data) => success_frame(request.id, &data),
                Err((name, message)) => error_frame(request.id, &name, &message),
            },
            None => error_frame(
                request.id,
                "NotImplemented",
                &format!(
                    "Host does not implement channel `{}` yet (implemented: {})",
                    request.channel,
                    channels.names().join(", ")
                ),
            ),
        },
        REQUEST_EVENT_LISTEN => error_frame(
            request.id,
            "NotImplemented",
            &format!(
                "Host does not implement events for channel `{}` yet",
                request.channel
            ),
        ),
        // 取消/注销：没有长任务与事件订阅，等价 no-op（TS 客户端按需忽略）。
        REQUEST_PROMISE_CANCEL | REQUEST_EVENT_DISPOSE => Vec::new(),
        other => error_frame(
            request.id,
            "InvalidRequest",
            &format!("Unsupported RPC request type {other}"),
        ),
    }
}

/// `settingService`（TS `ServiceChannels.Setting` = `"setting"`）：
/// `get` 读设置文件、`update` 顶层合并补丁后原子写（与 Node Host 同文件同格式）。
/// 其余方法（`updateDataBaseDir` / `ensureDefaultProject`）回显式缺口。
pub fn setting_channel(home: std::path::PathBuf) -> (&'static str, ChannelHandler) {
    let handler: ChannelHandler = Arc::new(move |method: String, arg: Wire| {
        let home = home.clone();
        Box::pin(async move {
            match method.as_str() {
                "get" => escode_host_services::settings::read(&home)
                    .await
                    .map(Wire::Object)
                    .map_err(|error| ("Fatal".to_owned(), format!("{error:#}"))),
                "update" => {
                    // `ProxyChannel.toService` 把实参打包成数组：`update(patch)` → `[patch]`。
                    let patch = arg
                        .into_args()
                        .into_iter()
                        .next()
                        .and_then(Wire::into_object)
                        .ok_or_else(|| {
                            (
                                "InvalidRequest".to_owned(),
                                "setting.update requires a patch object".to_owned(),
                            )
                        })?;
                    escode_host_services::settings::apply_patch(&home, &patch)
                        .await
                        .map(|_| Wire::Undefined)
                        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))
                }
                other => Err((
                    "NotImplemented".to_owned(),
                    format!("Host does not implement `setting.{other}` yet"),
                )),
            }
        })
    });
    ("setting", handler)
}

/// 组合根登记全部已迁移的 channel（Host 进程一处装配）。
pub fn channels(home: std::path::PathBuf) -> Channels {
    Channels::new(vec![setting_channel(home)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_fixture(kind: i64, id: i64, channel: &str, method: &str, arg: &Wire) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str(channel.to_owned()),
            Wire::Str(method.to_owned()),
        ]));
        writer.value(arg);
        writer.into_bytes()
    }

    #[tokio::test]
    async fn unknown_channel_reports_an_explicit_gap() {
        let channels = channels(std::path::PathBuf::from("/nonexistent"));
        let body = request_fixture(REQUEST_PROMISE, 9, "workspace", "query", &Wire::Undefined);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let header = reader.value().unwrap();
        let payload = reader.value().unwrap();
        let Wire::Array(items) = header else {
            panic!("response header must be an array")
        };
        assert_eq!(items[0].as_int(), Some(RESPONSE_PROMISE_ERROR));
        assert_eq!(items[1].as_int(), Some(9));
        let Wire::Object(error) = payload else {
            panic!("error payload must be an object")
        };
        assert_eq!(error["name"], "NotImplemented");
        assert!(error["message"].as_str().unwrap().contains("workspace"));
        assert!(error["message"].as_str().unwrap().contains("setting"));
    }

    #[tokio::test]
    async fn settings_get_and_update_round_trip_over_the_wire() {
        let dir = tempfile::tempdir().unwrap();
        let channels = channels(dir.path().to_path_buf());
        let body = request_fixture(REQUEST_PROMISE, 3, "setting", "get", &Wire::Undefined);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let Wire::Array(header) = reader.value().unwrap() else {
            panic!("header")
        };
        assert_eq!(header[0].as_int(), Some(RESPONSE_PROMISE_SUCCESS));
        assert_eq!(reader.value().unwrap(), Wire::Object(json!({})));

        // 与 ProxyChannel.toService 同形：实参打包成数组。
        let patch = Wire::Array(vec![Wire::Object(json!({"locale": "zh-CN"}))]);
        let body = request_fixture(REQUEST_PROMISE, 4, "setting", "update", &patch);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let Wire::Array(header) = reader.value().unwrap() else {
            panic!("header")
        };
        assert_eq!(header[0].as_int(), Some(RESPONSE_PROMISE_SUCCESS));
        assert_eq!(reader.value().unwrap(), Wire::Undefined);
        // 落盘可由下一次 get 读到（同文件同格式）。
        let body = request_fixture(REQUEST_PROMISE, 5, "setting", "get", &Wire::Undefined);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let _ = reader.value().unwrap();
        let Wire::Object(settings) = reader.value().unwrap() else {
            panic!("settings object")
        };
        assert_eq!(settings["locale"], "zh-CN");
    }

    #[tokio::test]
    async fn cancel_requests_have_no_response() {
        let channels = channels(std::path::PathBuf::from("/nonexistent"));
        let body = request_fixture(
            REQUEST_PROMISE_CANCEL,
            1,
            "setting",
            "get",
            &Wire::Undefined,
        );
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        assert!(response.is_empty());
    }

    #[test]
    fn initialize_frame_uses_response_type_200() {
        let frame = initialize_frame();
        let mut reader = Reader::new(&frame);
        let Wire::Array(header) = reader.value().unwrap() else {
            panic!("header")
        };
        assert_eq!(header, vec![Wire::Int(RESPONSE_INITIALIZE)]);
        assert_eq!(reader.value().unwrap(), Wire::Undefined);
    }
}
