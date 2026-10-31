//! 远端 server 的 stdio 握手（与 `packages/server/src/remote/handshake.ts` 对等）。
//!
//! 远端 `escode-server` 启动后先输出一行 `{"type":"escode-hello",...}`；客户端回
//! `{"type":"escode-hello-ack","version":...,"clientId":...}`，之后的字节流才是
//! [`crate::rpc_wire`] 的 RPC 帧面。
//!
//! SSH banner / MOTD 可能出现在 hello 之前：非 JSON 行一律跳过；JSON 但类型不是 hello
//! 的行同样跳过（与 TS 一致）。hello 字段不合法时显式报错，不继续读帧。

use std::fmt;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::source_server_compatibility::CompatibilityManifest;

/// 诊断信息上限（与 TS `MAX_HANDSHAKE_DIAGNOSTIC_CHARS` 一致）。
pub const MAX_DIAGNOSTIC_CHARS: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub pid: i64,
    /// 远端兼容清单；旧 server 不返回时为 `None`（兼容未知，不据此拒连）。
    pub compatibility: Option<CompatibilityManifest>,
}

/// 握手成功：hello 内容 + 同一次读取里 hello 行之后的剩余字节。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    pub hello: Hello,
    pub remaining: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandshakeError {
    pub message: String,
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HandshakeError {}

/// 增量握手：把 stdout 片段依次 `push`，直到返回 `Some(Handshake)`。
#[derive(Default)]
pub struct HandshakeReader {
    buffer: Vec<u8>,
    diagnostics: String,
}

impl HandshakeReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
        self.append_diagnostics(chunk);
    }

    /// 已缓存但未消费的字节数（hello 行之后的数据会在成功后返回给调用方）。
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// 解析 hello；不足一行返回 `Ok(None)`，成功时消费 hello 行并返回剩余字节。
    pub fn poll(&mut self) -> Result<Option<Handshake>, HandshakeError> {
        loop {
            let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') else {
                return Ok(None);
            };
            let line = String::from_utf8_lossy(&self.buffer[..newline])
                .trim()
                .to_owned();
            let consumed = newline + 1;
            if !line.starts_with('{') {
                self.buffer.drain(..consumed);
                continue;
            }
            let value: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => {
                    // 不是合法 JSON（例如 MOTD 以 `{` 开头）：与 TS 一样跳过。
                    self.buffer.drain(..consumed);
                    continue;
                }
            };
            if value.get("type").and_then(Value::as_str) != Some("escode-hello") {
                self.buffer.drain(..consumed);
                continue;
            }
            let hello = parse_hello(&value).map_err(|error| HandshakeError {
                message: format!("{error}; stdout tail: {}", self.diagnostics),
            })?;
            self.buffer.drain(..consumed);
            let remaining = std::mem::take(&mut self.buffer);
            return Ok(Some(Handshake { hello, remaining }));
        }
    }

    fn append_diagnostics(&mut self, chunk: &[u8]) {
        self.diagnostics.push_str(&String::from_utf8_lossy(chunk));
        if self.diagnostics.len() > MAX_DIAGNOSTIC_CHARS {
            let cut = self.diagnostics.len() - MAX_DIAGNOSTIC_CHARS;
            let cut = (cut..self.diagnostics.len())
                .find(|index| self.diagnostics.is_char_boundary(*index))
                .unwrap_or(self.diagnostics.len());
            self.diagnostics.drain(..cut);
        }
    }
}

fn parse_hello(value: &Value) -> Result<Hello> {
    let field = |name: &str| -> Result<String> {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("invalid escode-hello: missing {name}"))
    };
    let pid = value
        .get("pid")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("invalid escode-hello: missing pid"))?;
    let compatibility =
        match value.get("compatibility") {
            None | Some(Value::Null) => None,
            Some(raw) => Some(CompatibilityManifest::parse(raw).map_err(|error| {
                anyhow::anyhow!("invalid escode-hello compatibility: {error:#}")
            })?),
        };
    Ok(Hello {
        version: field("version")?,
        platform: field("platform")?,
        arch: field("arch")?,
        pid,
        compatibility,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HelloAck<'a> {
    r#type: &'static str,
    version: &'a str,
    client_id: &'a str,
}

/// `escode-hello-ack` 行（含结尾换行），字段顺序与 TS `JSON.stringify` 一致。
pub fn hello_ack_line(version: &str, client_id: &str) -> Result<String> {
    if client_id.trim().is_empty() {
        bail!("clientId must not be empty");
    }
    let ack = HelloAck {
        r#type: "escode-hello-ack",
        version,
        client_id,
    };
    Ok(format!("{}\n", serde_json::to_string(&ack)?))
}

#[cfg(test)]
#[path = "source_server_handshake_tests.rs"]
mod tests;
