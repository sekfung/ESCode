//! 处理函数 panic 的响应形状（rust-u2-host-resilience.md §5.1）：
//! 同步前缀与异步体都必须回一条 `Fatal` 错误帧，不能让 Promise 请求悬空。

use std::future::Future;
use std::pin::Pin;

use super::{ChannelResult, REQUEST_PROMISE, RESPONSE_PROMISE_ERROR, Request, respond};
use crate::channel::{Channel, Channels, handler};
use crate::wire::{Reader, Wire};

fn request(id: i64, method: &str) -> Request {
    Request {
        kind: REQUEST_PROMISE,
        id,
        channel: "test".to_owned(),
        method: method.to_owned(),
        arg: Wire::Undefined,
    }
}

fn decode_error(body: &[u8]) -> (i64, i64, String, String) {
    let mut reader = Reader::new(body);
    let header = reader.value().expect("response header");
    let payload = reader.value().expect("response payload");
    let Wire::Array(items) = header else {
        panic!("response header must be an array");
    };
    let kind = items[0].as_int().expect("response type");
    let id = items[1].as_int().expect("response id");
    let Wire::Object(object) = payload else {
        panic!("error payload must be an object");
    };
    let name = object["name"].as_str().expect("error name").to_owned();
    let message = object["message"]
        .as_str()
        .expect("error message")
        .to_owned();
    (id, kind, name, message)
}

#[tokio::test]
async fn synchronous_handler_panic_becomes_a_fatal_error_frame() {
    let channels = Channels::new(vec![Channel::new(
        "test",
        vec![(
            "syncPanic",
            handler(
                |_arg| -> Pin<Box<dyn Future<Output = ChannelResult> + Send>> {
                    panic!("sync boom")
                },
            ),
        )],
    )]);
    let body = respond(&channels, &request(7, "syncPanic")).await;
    let (id, kind, name, message) = decode_error(&body);
    assert_eq!((id, kind), (7, RESPONSE_PROMISE_ERROR));
    assert_eq!(name, "Fatal");
    assert!(message.contains("test.syncPanic"), "{message}");
    assert!(message.contains("sync boom"), "{message}");
}

async fn async_panic() -> ChannelResult {
    panic!("async boom")
}

#[tokio::test]
async fn asynchronous_handler_panic_becomes_a_fatal_error_frame() {
    let channels = Channels::new(vec![Channel::new(
        "test",
        vec![("asyncPanic", handler(|_arg| Box::pin(async_panic())))],
    )]);
    let body = respond(&channels, &request(8, "asyncPanic")).await;
    let (id, kind, name, message) = decode_error(&body);
    assert_eq!((id, kind), (8, RESPONSE_PROMISE_ERROR));
    assert_eq!(name, "Fatal");
    assert!(message.contains("test.asyncPanic"), "{message}");
    assert!(message.contains("async boom"), "{message}");
}

#[test]
fn panic_message_accepts_str_and_string_payloads() {
    let string = String::from("owned");
    assert_eq!(super::panic_message(&string), "owned");
}
