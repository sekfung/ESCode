//! RPC 值序列化（与 `packages/rpc/src/serialization.ts` 对等）。
//!
//! 格式：`[1 byte 类型] [VQL 长度(按需)] [数据]`，类型标签与 VQL 语义逐字节对齐 TS：
//!
//! ```text
//! 0 Undefined | 1 String | 2 Buffer | 3 VSBuffer
//! 4 Array     | 5 Object(JSON) | 6 Int(VQL, i32)
//! ```
//!
//! 对象里的嵌套 `Uint8Array` 在 TS 侧用 `{__escode_rpc_nested_uint8array_v1, base64}`
//! 标记；这里保持 JSON 原样，由使用方在需要二进制时再解码（不在这一层猜类型）。

use std::fmt;

use serde_json::Value;

pub const TAG_UNDEFINED: u8 = 0;
pub const TAG_STRING: u8 = 1;
pub const TAG_BUFFER: u8 = 2;
pub const TAG_VS_BUFFER: u8 = 3;
pub const TAG_ARRAY: u8 = 4;
pub const TAG_OBJECT: u8 = 5;
pub const TAG_INT: u8 = 6;

/// 二进制载荷的原始类型（TS 区分 `Uint8Array` 与 `VSBuffer`，解码端都按字节处理）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BytesKind {
    Buffer,
    VsBuffer,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WireValue {
    Undefined,
    String(String),
    Bytes {
        kind: BytesKind,
        bytes: Vec<u8>,
    },
    Array(Vec<WireValue>),
    /// `null` / bool / 非 i32 数字 / 普通对象都走 JSON fallback（与 TS 一致）。
    Object(Value),
    Int(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SerializationError {
    Truncated { at: usize, needed: usize },
    UnknownTag(u8),
    VqlTooLong,
    InvalidJson(String),
}

impl fmt::Display for SerializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { at, needed } => {
                write!(
                    formatter,
                    "truncated rpc value at {at}, need {needed} bytes"
                )
            }
            Self::UnknownTag(tag) => write!(formatter, "unknown rpc value tag {tag}"),
            Self::VqlTooLong => formatter.write_str("VQL integer exceeds 5 bytes"),
            Self::InvalidJson(error) => write!(formatter, "invalid rpc object json: {error}"),
        }
    }
}

impl std::error::Error for SerializationError {}

impl WireValue {
    pub fn from_json(value: &Value) -> Self {
        match value {
            Value::String(text) => Self::String(text.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from_json).collect()),
            Value::Number(number) => {
                match number.as_i64().and_then(|int| i32::try_from(int).ok()) {
                    Some(int) => Self::Int(int),
                    None => Self::Object(value.clone()),
                }
            }
            _ => Self::Object(value.clone()),
        }
    }

    /// 转回 JSON；`Undefined` 无法表示，返回 `None`（调用方按“省略参数”处理）。
    pub fn to_json(&self) -> Option<Value> {
        match self {
            Self::Undefined => None,
            Self::String(text) => Some(Value::String(text.clone())),
            Self::Int(int) => Some(Value::from(*int)),
            Self::Object(value) => Some(value.clone()),
            Self::Array(items) => Some(Value::Array(
                items.iter().filter_map(Self::to_json).collect(),
            )),
            Self::Bytes { bytes, .. } => Some(Value::Array(
                bytes.iter().map(|byte| Value::from(*byte)).collect(),
            )),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Self::Undefined => out.push(TAG_UNDEFINED),
            Self::String(text) => {
                out.push(TAG_STRING);
                write_vql(out, text.len() as u32);
                out.extend_from_slice(text.as_bytes());
            }
            Self::Bytes { kind, bytes } => {
                out.push(match kind {
                    BytesKind::Buffer => TAG_BUFFER,
                    BytesKind::VsBuffer => TAG_VS_BUFFER,
                });
                write_vql(out, bytes.len() as u32);
                out.extend_from_slice(bytes);
            }
            Self::Array(items) => {
                out.push(TAG_ARRAY);
                write_vql(out, items.len() as u32);
                for item in items {
                    item.encode_into(out);
                }
            }
            Self::Object(value) => {
                let json = value.to_string();
                out.push(TAG_OBJECT);
                write_vql(out, json.len() as u32);
                out.extend_from_slice(json.as_bytes());
            }
            Self::Int(int) => {
                out.push(TAG_INT);
                write_vql(out, *int as u32);
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), SerializationError> {
        let mut reader = Reader { bytes, offset: 0 };
        let value = reader.value()?;
        Ok((value, reader.offset))
    }

    /// 解码恰好一整个值；尾部还有字节视为错误（帧内必须整段消费）。
    pub fn decode_exact(bytes: &[u8]) -> Result<Self, SerializationError> {
        let (value, used) = Self::decode(bytes)?;
        if used != bytes.len() {
            return Err(SerializationError::Truncated {
                at: used,
                needed: bytes.len() - used,
            });
        }
        Ok(value)
    }
}

/// VQL（7 bit/字节，最高位表示后续字节）；与 TS `writeInt32VQL` 一致。
pub fn write_vql(out: &mut Vec<u8>, value: u32) {
    let mut value = value;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

pub fn read_vql(bytes: &[u8], offset: &mut usize) -> Result<u32, SerializationError> {
    let mut value: u32 = 0;
    for shift in (0..=28).step_by(7) {
        let byte = *bytes.get(*offset).ok_or(SerializationError::Truncated {
            at: *offset,
            needed: 1,
        })?;
        *offset += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(SerializationError::VqlTooLong)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], SerializationError> {
        let end = self.offset + count;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or(SerializationError::Truncated {
                at: self.offset,
                needed: count,
            })?;
        self.offset = end;
        Ok(slice)
    }

    fn length(&mut self) -> Result<usize, SerializationError> {
        Ok(read_vql(self.bytes, &mut self.offset)? as usize)
    }

    fn value(&mut self) -> Result<WireValue, SerializationError> {
        let tag = self.take(1)?[0];
        match tag {
            TAG_UNDEFINED => Ok(WireValue::Undefined),
            TAG_STRING => {
                let length = self.length()?;
                let bytes = self.take(length)?;
                Ok(WireValue::String(
                    String::from_utf8_lossy(bytes).into_owned(),
                ))
            }
            TAG_BUFFER | TAG_VS_BUFFER => {
                let length = self.length()?;
                let bytes = self.take(length)?.to_vec();
                Ok(WireValue::Bytes {
                    kind: if tag == TAG_BUFFER {
                        BytesKind::Buffer
                    } else {
                        BytesKind::VsBuffer
                    },
                    bytes,
                })
            }
            TAG_ARRAY => {
                let length = self.length()?;
                let mut items = Vec::with_capacity(length.min(1024));
                for _ in 0..length {
                    items.push(self.value()?);
                }
                Ok(WireValue::Array(items))
            }
            TAG_OBJECT => {
                let length = self.length()?;
                let bytes = self.take(length)?;
                let value = serde_json::from_slice(bytes)
                    .map_err(|error| SerializationError::InvalidJson(error.to_string()))?;
                Ok(WireValue::Object(value))
            }
            TAG_INT => Ok(WireValue::Int(
                read_vql(self.bytes, &mut self.offset)? as i32
            )),
            other => Err(SerializationError::UnknownTag(other)),
        }
    }
}

#[cfg(test)]
#[path = "rpc_serialization_tests.rs"]
mod tests;
