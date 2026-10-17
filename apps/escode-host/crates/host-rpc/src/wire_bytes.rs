//! `@escode/rpc` 的二进制参数/结果编码（`packages/rpc/src/serialization.ts`）。
//!
//! 对象里的 `Uint8Array` 走 JSON fallback，编码成
//! `{__escode_rpc_nested_uint8array_v1: true, base64: "..."}`，反序列化时恢复为 Uint8Array。
//! 顶层 Uint8Array 走二进制帧，本 Host 的服务参数/结果都在对象里，因此只需要嵌套形态。

use base64::Engine as _;
use serde_json::{Value, json};

const MARKER: &str = "__escode_rpc_nested_uint8array_v1";
const BASE64: &str = "base64";

/// 解码嵌套 `Uint8Array`；纯数字数组（测试/JSON 直发）也接受，`null` 视为空。
pub fn decode(value: &Value) -> Result<Vec<u8>, String> {
    if value.get(MARKER).and_then(Value::as_bool) == Some(true)
        && let Some(text) = value.get(BASE64).and_then(Value::as_str)
    {
        return base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|error| format!("Invalid bytes: {error}"));
    }
    if let Some(items) = value.as_array() {
        return Ok(items
            .iter()
            .filter_map(Value::as_u64)
            .map(|byte| byte as u8)
            .collect());
    }
    if value.is_null() {
        return Ok(Vec::new());
    }
    Err("bytes must be a Uint8Array".to_owned())
}

/// 编码成协议标记（TS `encodeRpcJsonValue`）。
pub fn encode(bytes: &[u8]) -> Value {
    json!({
        MARKER: true,
        BASE64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}
