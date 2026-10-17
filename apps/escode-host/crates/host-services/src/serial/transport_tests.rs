//! 配置解析与打开错误映射（`rust-u2-serial.md` §9）。
use super::*;

fn config(value: &serde_json::Value) -> Result<SerialConfig, SerialOpenError> {
    SerialConfig::from_json(value)
}

#[test]
fn config_round_trips_and_validates() {
    let raw = serde_json::json!({
        "baudRate": 115200, "dataBits": 8, "parity": "none",
        "stopBits": 1, "rtscts": false, "autoReconnect": true
    });
    let parsed = config(&raw).unwrap();
    assert_eq!(parsed.baud_rate, 115200);
    assert!(parsed.auto_reconnect);
    let rendered = parsed.to_json();
    assert_eq!(rendered["stopBits"], serde_json::json!(1.0));
    assert_eq!(rendered["autoReconnect"], serde_json::json!(true));
    assert_eq!(config(&rendered).unwrap(), parsed, "往返一致");

    for (label, value) in [
        (
            "缺波特率",
            serde_json::json!({"dataBits": 8, "parity": "none", "stopBits": 1}),
        ),
        (
            "数据位非法",
            serde_json::json!({"baudRate": 9600, "dataBits": 9, "parity": "none", "stopBits": 1}),
        ),
        (
            "校验非法",
            serde_json::json!({"baudRate": 9600, "dataBits": 8, "parity": "x", "stopBits": 1}),
        ),
        (
            "停止位非法",
            serde_json::json!({"baudRate": 9600, "dataBits": 8, "parity": "none", "stopBits": 3}),
        ),
    ] {
        let error = config(&value).unwrap_err();
        assert_eq!(error.code, "invalidConfig", "{label}");
    }
    // 多个问题一起报（与 TS 拼接顺序一致）。
    let error =
        config(&serde_json::json!({"dataBits": 9, "parity": "x", "stopBits": 3})).unwrap_err();
    assert_eq!(
        error.message,
        "Invalid serial config: baudRate, dataBits, parity, stopBits"
    );
}

#[test]
fn open_errors_map_to_stable_codes() {
    for (message, code) in [
        ("Access denied", "busy"),
        ("Cannot lock port", "busy"),
        ("EBUSY: resource busy", "busy"),
        ("EACCES: permission denied", "denied"),
        ("ENOENT: no such file or directory", "notFound"),
        ("EINVAL: invalid argument", "invalidConfig"),
        ("something else", "io"),
    ] {
        assert_eq!(
            SerialOpenError::from_message(message.to_owned()).code,
            code,
            "{message}"
        );
    }
}
