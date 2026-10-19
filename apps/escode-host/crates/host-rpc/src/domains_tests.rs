//! `domains` 组合根的单测（channels 装配、setting 读写、缺口回显）。

use super::*;
use crate::session::{
    REQUEST_PROMISE, REQUEST_PROMISE_CANCEL, RESPONSE_INITIALIZE, RESPONSE_PROMISE_ERROR,
    RESPONSE_PROMISE_SUCCESS, initialize_frame, parse_request, respond,
};
use crate::wire::{Reader, Writer};
use serde_json::json;

fn test_channels(home: std::path::PathBuf) -> Channels {
    let options: escode_host_surface::OptionsFactory =
        std::sync::Arc::new(|workspace: &escode_host_control::WorkspaceDescriptor| {
            escode_cli_rust::embed::RuntimeOptions {
                home: std::path::PathBuf::from("/tmp"),
                env: std::collections::BTreeMap::new(),
                argv: Vec::new(),
                cwd: std::path::PathBuf::from(&workspace.path),
                workspace_identity: workspace.identity.clone(),
                data_dir: None,
                config: None,
            }
        });
    channels(
        home.clone(),
        Some(home),
        escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded),
        options,
    )
}

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
    let channels = test_channels(std::path::PathBuf::from("/nonexistent"));
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
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("escode-session"),
        "已实现清单应包含 escode-session: {error}"
    );
}

#[tokio::test]
async fn settings_get_and_update_round_trip_over_the_wire() {
    let dir = tempfile::tempdir().unwrap();
    let channels = test_channels(dir.path().to_path_buf());
    let body = request_fixture(REQUEST_PROMISE, 3, "setting", "get", &Wire::Undefined);
    let response = respond(&channels, &parse_request(&body).unwrap()).await;
    let mut reader = Reader::new(&response);
    let Wire::Array(header) = reader.value().unwrap() else {
        panic!("header")
    };
    assert_eq!(header[0].as_int(), Some(RESPONSE_PROMISE_SUCCESS));
    let Wire::Object(settings) = reader.value().unwrap() else {
        panic!("settings object")
    };
    assert_eq!(settings, escode_host_services::settings::default_settings());

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
    let channels = test_channels(std::path::PathBuf::from("/nonexistent"));
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
