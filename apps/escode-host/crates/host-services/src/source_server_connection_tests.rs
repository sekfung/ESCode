#![cfg(unix)]

//! S4-h：握手/EOF 失败时把 transport 的 stderr 带进错误链，供 `remote-host-key-changed` 归因。

use std::path::PathBuf;

use super::{ConnectOptions, connect};
use crate::source_server_transport::{TransportCommand, is_host_key_changed_error};

fn fake_transport(script: &str) -> TransportCommand {
    TransportCommand {
        program: PathBuf::from("sh"),
        args: vec!["-c".into(), script.into()],
        env: std::collections::BTreeMap::new(),
    }
}

/// OpenSSH 在握手前因 host key 变化退出：stderr 诊断必须保留在错误里。
#[tokio::test]
async fn host_key_diagnostics_reach_the_handshake_error() {
    let command =
        fake_transport("echo 'WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!' >&2; exit 255");
    let options = ConnectOptions::new("client", "3.16.2");
    let error = match connect(&command, &options).await {
        Ok(_) => panic!("transport without hello must fail"),
        Err(error) => error,
    };
    let text = format!("{error:#}");
    assert!(text.contains("remote stderr:"), "{text}");
    assert!(is_host_key_changed_error(&text), "{text}");
}

/// 普通认证失败不能被误判成 host key 变化。
#[tokio::test]
async fn authentication_failure_is_not_a_host_key_change() {
    let command = fake_transport("echo 'Permission denied (publickey).' >&2; exit 255");
    let options = ConnectOptions::new("client", "3.16.2");
    let error = match connect(&command, &options).await {
        Ok(_) => panic!("transport without hello must fail"),
        Err(error) => error,
    };
    let text = format!("{error:#}");
    assert!(text.contains("Permission denied"), "{text}");
    assert!(!is_host_key_changed_error(&text), "{text}");
}
