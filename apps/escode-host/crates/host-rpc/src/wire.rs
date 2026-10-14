//! `@escode/rpc` 的二进制序列化（packages/rpc/src/serialization.ts）。
//!
//! 格式：`[1 byte 类型标签] [VQL 长度] [数据]`；VQL 每字节 7 bit，最高位标记后面还有字节。
//! 对象走 JSON 字符串（TS 对嵌套 `Uint8Array` 会加 base64 标记；Rust 侧不消费二进制 JSON
//! 字段，标记对象原样保留，往返不破坏）。
//!
//! 服务端只发送客户端会消费的类型（Array/Int/String/Object/Undefined），接收端能解出
//! 客户端可能发来的全部类型（含 Buffer/VSBuffer）。

use serde_json::Value;

/// TS `DataType` 标签。
const TYPE_UNDEFINED: u8 = 0;
const TYPE_STRING: u8 = 1;
const TYPE_BUFFER: u8 = 2;
const TYPE_VS_BUFFER: u8 = 3;
const TYPE_ARRAY: u8 = 4;
const TYPE_OBJECT: u8 = 5;
const TYPE_INT: u8 = 6;

/// 线上值：与 TS `serialize`/`deserialize` 的类型集合一一对应。
#[derive(Clone, Debug, PartialEq)]
pub enum Wire {
    Undefined,
    Str(String),
    /// `Uint8Array`（TS `Buffer` 标签）与 `VSBuffer` 都归到字节串；`vs` 只影响回发时的标签。
    Bytes {
        vs: bool,
        data: Vec<u8>,
    },
    Array(Vec<Wire>),
    Object(Value),
    Int(i64),
}

impl Wire {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn into_object(self) -> Option<Value> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    /// `ProxyChannel.toService` 把 JS 实参打包成数组后作为 arg 发送（`[...args]`）。
    pub fn into_args(self) -> Vec<Wire> {
        match self {
            Self::Array(items) => items,
            Self::Undefined => Vec::new(),
            other => vec![other],
        }
    }
}

/// 解码游标：越界即错误（半截帧不会被当成有效消息）。
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// 已消费的字节数（测试与诊断用）。
    pub fn consumed(&self) -> usize {
        self.pos
    }

    fn take(&mut self, count: usize) -> anyhow::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| anyhow::anyhow!("RPC payload truncated"))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn byte(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// VQL：7 bit/字节，最高位表示后续还有。
    fn vql(&mut self) -> anyhow::Result<u64> {
        let mut value: u64 = 0;
        for shift in (0..64).step_by(7) {
            let next = self.byte()?;
            value |= u64::from(next & 0b0111_1111) << shift;
            if next & 0b1000_0000 == 0 {
                return Ok(value);
            }
        }
        anyhow::bail!("RPC VQL length exceeds 64 bits");
    }

    /// 读一个 `serialize` 值。
    pub fn value(&mut self) -> anyhow::Result<Wire> {
        match self.byte()? {
            TYPE_UNDEFINED => Ok(Wire::Undefined),
            TYPE_STRING => {
                let len = self.vql()? as usize;
                let bytes = self.take(len)?;
                Ok(Wire::Str(String::from_utf8_lossy(bytes).into_owned()))
            }
            tag @ (TYPE_BUFFER | TYPE_VS_BUFFER) => {
                let len = self.vql()? as usize;
                let data = self.take(len)?.to_vec();
                Ok(Wire::Bytes {
                    vs: tag == TYPE_VS_BUFFER,
                    data,
                })
            }
            TYPE_ARRAY => {
                let count = self.vql()? as usize;
                let mut items = Vec::with_capacity(count.min(64));
                for _ in 0..count {
                    items.push(self.value()?);
                }
                Ok(Wire::Array(items))
            }
            TYPE_OBJECT => {
                let len = self.vql()? as usize;
                let bytes = self.take(len)?;
                let text = String::from_utf8_lossy(bytes);
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| anyhow::anyhow!("RPC object payload is not JSON: {error}"))?;
                Ok(Wire::Object(value))
            }
            TYPE_INT => Ok(Wire::Int(self.vql()? as i64)),
            other => anyhow::bail!("Unsupported RPC data type tag: {other}"),
        }
    }
}

/// 写端：把值按 TS `serialize` 编码。
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    fn vql(&mut self, mut value: u64) {
        if value == 0 {
            self.bytes.push(0);
            return;
        }
        while value != 0 {
            let mut byte = (value & 0b0111_1111) as u8;
            value >>= 7;
            if value > 0 {
                byte |= 0b1000_0000;
            }
            self.bytes.push(byte);
        }
    }

    pub fn value(&mut self, value: &Wire) {
        match value {
            Wire::Undefined => self.bytes.push(TYPE_UNDEFINED),
            Wire::Str(text) => {
                self.bytes.push(TYPE_STRING);
                self.vql(text.len() as u64);
                self.bytes.extend_from_slice(text.as_bytes());
            }
            Wire::Bytes { vs, data } => {
                self.bytes
                    .push(if *vs { TYPE_VS_BUFFER } else { TYPE_BUFFER });
                self.vql(data.len() as u64);
                self.bytes.extend_from_slice(data);
            }
            Wire::Array(items) => {
                self.bytes.push(TYPE_ARRAY);
                self.vql(items.len() as u64);
                for item in items {
                    self.value(item);
                }
            }
            Wire::Object(value) => {
                let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned());
                self.bytes.push(TYPE_OBJECT);
                self.vql(text.len() as u64);
                self.bytes.extend_from_slice(text.as_bytes());
            }
            // TS 的整数走 VQL；超出 32 位的值退回 Object（JSON）保证往返不丢精度
            // （TS 的 `value | 0` 同样只把 32 位整数当 Int）。
            Wire::Int(value) if (0..=u32::MAX as i64).contains(value) => {
                self.bytes.push(TYPE_INT);
                self.vql(*value as u64);
            }
            Wire::Int(value) => self.value(&Wire::Object(serde_json::json!(*value))),
        }
    }

    /// 便捷写法：整数（请求头与响应头里大量使用）。
    pub fn int(&mut self, value: i64) {
        self.value(&Wire::Int(value));
    }

    pub fn str(&mut self, value: &str) {
        self.value(&Wire::Str(value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: Wire) -> Wire {
        let mut writer = Writer::new();
        writer.value(&value);
        let bytes = writer.into_bytes();
        let mut reader = Reader::new(&bytes);
        let decoded = reader.value().expect("decodes");
        assert_eq!(reader.consumed(), bytes.len(), "整段被消费");
        decoded
    }

    #[test]
    fn vql_matches_the_ts_encoding() {
        // TS 例：0 → [0x00]，127 → [0x7F]，128 → [0x80, 0x01]。
        let mut writer = Writer::new();
        writer.int(0);
        assert_eq!(writer.into_bytes(), vec![TYPE_INT, 0x00]);
        let mut writer = Writer::new();
        writer.int(127);
        assert_eq!(writer.into_bytes(), vec![TYPE_INT, 0x7F]);
        let mut writer = Writer::new();
        writer.int(128);
        assert_eq!(writer.into_bytes(), vec![TYPE_INT, 0x80, 0x01]);
        let mut writer = Writer::new();
        writer.int(100);
        assert_eq!(writer.into_bytes(), vec![TYPE_INT, 100]);
    }

    #[test]
    fn values_round_trip() {
        assert_eq!(round_trip(Wire::Undefined), Wire::Undefined);
        assert_eq!(
            round_trip(Wire::Str("setting".to_owned())),
            Wire::Str("setting".to_owned())
        );
        assert_eq!(
            round_trip(Wire::Bytes {
                vs: true,
                data: vec![1, 2, 3]
            }),
            Wire::Bytes {
                vs: true,
                data: vec![1, 2, 3]
            }
        );
        let object = Wire::Object(serde_json::json!({"locale": "zh-CN", "nested": {"count": 2}}));
        assert_eq!(round_trip(object.clone()), object);
        let array = Wire::Array(vec![
            Wire::Int(100),
            Wire::Int(7),
            Wire::Str("setting".to_owned()),
            Wire::Str("get".to_owned()),
        ]);
        assert_eq!(round_trip(array.clone()), array);
    }

    #[test]
    fn request_fixture_matches_the_client_layout() {
        // 客户端实际发送的 Promise 请求：serialize([100, id, channel, method]) + serialize(arg)。
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(100),
            Wire::Int(1),
            Wire::Str("setting".to_owned()),
            Wire::Str("get".to_owned()),
        ]));
        writer.value(&Wire::Undefined);
        let bytes = writer.into_bytes();
        assert_eq!(
            bytes,
            vec![
                TYPE_ARRAY,
                4,
                TYPE_INT,
                100,
                TYPE_INT,
                1,
                TYPE_STRING,
                7,
                b's',
                b'e',
                b't',
                b't',
                b'i',
                b'n',
                b'g',
                TYPE_STRING,
                3,
                b'g',
                b'e',
                b't',
                TYPE_UNDEFINED,
            ]
        );
        let mut reader = Reader::new(&bytes);
        let header = reader.value().expect("header");
        let arg = reader.value().expect("arg");
        assert_eq!(arg, Wire::Undefined);
        let Wire::Array(items) = header else {
            panic!("header must be an array");
        };
        assert_eq!(items[0].as_int(), Some(100));
        assert_eq!(items[2].as_str(), Some("setting"));
        assert_eq!(items[3].as_str(), Some("get"));
    }

    #[test]
    fn truncated_payloads_are_rejected() {
        let mut writer = Writer::new();
        writer.value(&Wire::Str("setting".to_owned()));
        let mut bytes = writer.into_bytes();
        bytes.pop();
        assert!(Reader::new(&bytes).value().is_err(), "半截串必须失败");
    }
}
