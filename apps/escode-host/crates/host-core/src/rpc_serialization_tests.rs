use serde_json::json;

use super::{BytesKind, SerializationError, WireValue, read_vql, write_vql};

fn vql(value: u32) -> Vec<u8> {
    let mut out = Vec::new();
    write_vql(&mut out, value);
    out
}

/// VQL 与 TS 的 `writeInt32VQL` 一致。
#[test]
fn encodes_vql_like_the_typescript_helper() {
    assert_eq!(vql(0), vec![0x00]);
    assert_eq!(vql(127), vec![0x7f]);
    assert_eq!(vql(128), vec![0x80, 0x01]);
    assert_eq!(vql(300), vec![0xac, 0x02]);
    for value in [0, 1, 127, 128, 300, 16_383, 16_384, u32::MAX] {
        let bytes = vql(value);
        let mut offset = 0;
        assert_eq!(read_vql(&bytes, &mut offset).unwrap(), value);
        assert_eq!(offset, bytes.len());
    }
}

/// 每种类型标签的字节形与 TS `serialize` 一致。
#[test]
fn encodes_each_type_tag() {
    assert_eq!(WireValue::Undefined.encode(), vec![0]);
    assert_eq!(
        WireValue::String("hi".into()).encode(),
        vec![1, 2, b'h', b'i']
    );
    assert_eq!(
        WireValue::Bytes {
            kind: BytesKind::Buffer,
            bytes: vec![0xde, 0xad],
        }
        .encode(),
        vec![2, 2, 0xde, 0xad]
    );
    assert_eq!(
        WireValue::Bytes {
            kind: BytesKind::VsBuffer,
            bytes: vec![1],
        }
        .encode(),
        vec![3, 1, 1]
    );
    assert_eq!(
        WireValue::Array(vec![WireValue::Int(1), WireValue::String("a".into())]).encode(),
        vec![4, 2, 6, 1, 1, 1, b'a']
    );
    assert_eq!(
        WireValue::Object(json!({"a":1})).encode(),
        vec![5, 7, b'{', b'"', b'a', b'"', b':', b'1', b'}']
    );
    assert_eq!(WireValue::Int(42).encode(), vec![6, 42]);
    assert_eq!(
        WireValue::Int(-1).encode(),
        vec![6, 0xff, 0xff, 0xff, 0xff, 0x0f]
    );
}

/// 字符串长度是 UTF-8 字节数，不是字符数。
#[test]
fn string_length_is_utf8_bytes() {
    let encoded = WireValue::String("中文".into()).encode();
    assert_eq!(&encoded[..2], &[1, 6]);
    assert_eq!(encoded.len(), 8);
    assert_eq!(
        WireValue::decode_exact(&encoded).unwrap(),
        WireValue::String("中文".into())
    );
}

/// 全类型往返（含负数 int 与嵌套数组）。
#[test]
fn round_trips_nested_values() {
    let value = WireValue::Array(vec![
        WireValue::Undefined,
        WireValue::Int(-7),
        WireValue::String("x".into()),
        WireValue::Object(json!({"k": [1, true, null]})),
        WireValue::Bytes {
            kind: BytesKind::VsBuffer,
            bytes: vec![9, 8, 7],
        },
    ]);
    assert_eq!(WireValue::decode_exact(&value.encode()).unwrap(), value);
}

/// JSON 输入：i32 整数走 Int，非整数/布尔/null 走 Object；数组递归。
#[test]
fn maps_json_to_the_typescript_tags() {
    assert_eq!(WireValue::from_json(&json!(7)), WireValue::Int(7));
    assert_eq!(
        WireValue::from_json(&json!(7.5)),
        WireValue::Object(json!(7.5))
    );
    assert_eq!(
        WireValue::from_json(&json!(true)),
        WireValue::Object(json!(true))
    );
    assert_eq!(
        WireValue::from_json(&json!([1, "s"])),
        WireValue::Array(vec![WireValue::Int(1), WireValue::String("s".into())])
    );
    assert_eq!(
        WireValue::Int(7).to_json(),
        Some(json!(7)),
        "Int 回写为 JSON number"
    );
    assert_eq!(WireValue::Undefined.to_json(), None);
}

/// 截断/未知标签/超长 VQL 显式报错。
#[test]
fn rejects_malformed_values() {
    assert!(matches!(
        WireValue::decode(&[1, 5, b'a']),
        Err(SerializationError::Truncated { .. })
    ));
    assert_eq!(
        WireValue::decode(&[9]).unwrap_err(),
        SerializationError::UnknownTag(9)
    );
    assert_eq!(
        WireValue::decode(&[6, 0x80, 0x80, 0x80, 0x80, 0x80]).unwrap_err(),
        SerializationError::VqlTooLong
    );
    assert!(matches!(
        WireValue::decode_exact(&[6, 1, 0]),
        Err(SerializationError::Truncated { .. })
    ));
}
