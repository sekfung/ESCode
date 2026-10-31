use super::{HandshakeReader, hello_ack_line};

const HELLO: &str =
    r#"{"type":"escode-hello","version":"3.15.2","platform":"linux","arch":"x64","pid":4321}"#;

/// SSH banner / MOTD / 非 hello JSON 都被跳过；hello 之后的字节作为 remaining 返回。
#[test]
fn skips_banners_and_keeps_remaining_bytes() {
    let mut reader = HandshakeReader::new();
    reader.push(b"Welcome to Ubuntu\r\nLast login: now\r\n");
    reader.push(format!("{HELLO}\n").as_bytes());
    reader.push(b"payload-after-hello");
    let handshake = reader.poll().expect("handshake").expect("hello");
    assert_eq!(handshake.hello.version, "3.15.2");
    assert_eq!(handshake.hello.platform, "linux");
    assert_eq!(handshake.hello.arch, "x64");
    assert_eq!(handshake.hello.pid, 4321);
    assert_eq!(handshake.remaining, b"payload-after-hello".to_vec());
    assert_eq!(reader.buffered(), 0);
}

/// hello 行分片到达：不足一行返回 None，补齐后可解析。
#[test]
fn waits_for_a_split_hello_line() {
    let mut reader = HandshakeReader::new();
    let bytes = format!("{HELLO}\n").into_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        reader.push(std::slice::from_ref(byte));
        if index < bytes.len() - 1 {
            assert!(reader.poll().expect("no error").is_none(), "byte {index}");
        }
    }
    assert_eq!(
        reader.poll().expect("handshake").expect("hello").hello.pid,
        4321
    );
}

/// JSON 但字段缺失：显式报错，不把坏 hello 当 banner 跳过。
#[test]
fn rejects_invalid_hello_fields() {
    let mut reader = HandshakeReader::new();
    reader.push(br#"{"type":"escode-hello","version":"3.15.2"}"#);
    reader.push(b"\n");
    let error = reader.poll().expect_err("must fail");
    assert!(error.message.contains("invalid escode-hello"), "{error}");
    assert!(error.message.contains("stdout tail"), "{error}");
}

/// 非 hello 的 JSON 行（例如远端日志）跳过。
#[test]
fn skips_unrelated_json_lines() {
    let mut reader = HandshakeReader::new();
    reader.push(b"{\"level\":\"info\",\"msg\":\"booting\"}\n");
    reader.push(format!("{HELLO}\n").as_bytes());
    assert_eq!(
        reader
            .poll()
            .expect("handshake")
            .expect("hello")
            .hello
            .version,
        "3.15.2"
    );
}

/// ack 与 TS `JSON.stringify` 的字段顺序一致。
#[test]
fn writes_the_typescript_ack_shape() {
    assert_eq!(
        hello_ack_line("3.15.2", "desktop-1").expect("ack"),
        "{\"type\":\"escode-hello-ack\",\"version\":\"3.15.2\",\"clientId\":\"desktop-1\"}\n"
    );
    assert!(hello_ack_line("3.15.2", "  ").is_err());
}

/// 兼容清单是可选字段；旧 server 不带该字段仍可握手。
#[test]
fn legacy_hello_without_compatibility_is_accepted() {
    let mut reader = HandshakeReader::new();
    reader.push(format!("{HELLO}\n").as_bytes());
    let hello = reader.poll().expect("handshake").expect("hello").hello;
    assert_eq!(hello.compatibility, None);
}

/// 带兼容清单的 hello 被严格解析；字段值原样保留。
#[test]
fn parses_the_compatibility_manifest() {
    let mut reader = HandshakeReader::new();
    reader.push(
        br#"{"type":"escode-hello","version":"3.15.2","platform":"linux","arch":"x64","pid":1,"compatibility":{"schemaVersion":1,"productVersion":"3.15.2","minCompatibleVersion":"3.14.0"}}"#,
    );
    reader.push(b"\n");
    let hello = reader.poll().expect("handshake").expect("hello").hello;
    let compatibility = hello.compatibility.expect("compatibility");
    assert_eq!(compatibility.product_version, "3.15.2");
    assert_eq!(compatibility.min_compatible_version, "3.14.0");
}

/// 非法兼容清单显式报错，不能静默按「未知」继续。
#[test]
fn rejects_invalid_compatibility_manifest() {
    let mut reader = HandshakeReader::new();
    reader.push(
        br#"{"type":"escode-hello","version":"3.15.2","platform":"linux","arch":"x64","pid":1,"compatibility":{"schemaVersion":2,"productVersion":"3.15.2","minCompatibleVersion":"3.14.0"}}"#,
    );
    reader.push(b"\n");
    let error = reader.poll().expect_err("must fail");
    assert!(
        error.message.contains("invalid escode-hello compatibility"),
        "{error}"
    );
}
