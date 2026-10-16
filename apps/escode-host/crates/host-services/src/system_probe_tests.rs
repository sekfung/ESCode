use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_lowercase(), (*v).to_owned()))
        .collect();
    move |key| map.get(&key.to_lowercase()).cloned()
}

#[test]
fn non_windows_has_no_shell_options() {
    assert!(list_integrated_terminal_shells(false, &env_of(&[]), &|_| true).is_empty());
}

#[test]
fn windows_lists_cmd_and_git_bash_from_known_paths_or_path() {
    let env = env_of(&[
        ("ComSpec", " C:\\Windows\\System32\\cmd.exe "),
        ("PATH", "C:\\tools;C:\\Git\\cmd"),
    ]);
    let shells = list_integrated_terminal_shells(true, &env, &|p| {
        p == "C:\\Program Files\\Git\\bin\\bash.exe"
    });
    assert_eq!(shells[0]["id"], "cmd:C:\\Windows\\System32\\cmd.exe");
    assert_eq!(shells[1]["source"], "system");
    let shells = list_integrated_terminal_shells(true, &env, &|p| {
        p == "C:\\Git\\cmd\\git.exe" || p == "C:\\Git\\bin\\bash.exe"
    });
    assert_eq!(shells[1]["path"], "C:\\Git\\bin\\bash.exe");
    assert_eq!(shells[1]["source"], "path");
    let only_cmd = list_integrated_terminal_shells(true, &env_of(&[]), &|_| false);
    assert_eq!(only_cmd.len(), 1);
    assert_eq!(only_cmd[0]["path"], "cmd.exe", "无 ComSpec 回落 cmd.exe");
}

#[test]
fn targets_are_normalized_like_ts() {
    let Some(Target::Tcp {
        id,
        port,
        timeout_ms,
        ..
    }) = normalize_target(&json!({"host": " h ", "port": 99999, "timeoutMs": 5}))
    else {
        panic!("tcp")
    };
    assert_eq!((id.as_str(), port, timeout_ms), ("h:22", 22, 100));
    assert!(normalize_target(&json!({"host": "  "})).is_none());
    assert!(normalize_target(&json!({"kind": "service", "url": "ftp://x"})).is_none());
    assert!(normalize_target(&json!({"kind": "service", "url": "nope"})).is_none());
    let Some(Target::Service { id, marker, .. }) = normalize_target(
        &json!({"kind": "service", "url": "http://a.example", "expectedMarker": " m "}),
    ) else {
        panic!("service")
    };
    assert_eq!(
        (id.as_str(), marker.as_deref()),
        ("http://a.example/", Some("m"))
    );
}

async fn http_server(body: &'static str, status: &'static str) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn tcp_and_service_probes_report_reachability_and_markers() {
    let open = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let open_port = open.local_addr().expect("addr").port();
    let service = http_server(r#"{"ok":true,"marker":"zcode"}"#, "200 OK").await;
    let bad = http_server(r#"{"ok":false}"#, "200 OK").await;
    let result = probe_intranet(&json!({
        "targets": [
            {"host": "127.0.0.1", "port": open_port},
            {"host": "127.0.0.1", "port": 1, "timeoutMs": 200},
            {"kind": "service", "url": format!("http://{service}/"), "expectedMarker": "zcode"},
            {"kind": "service", "url": format!("http://{service}/"), "expectedMarker": "other"},
            {"kind": "service", "url": format!("http://{bad}/")},
        ],
        "attempts": 1, "requiredSuccessCount": 2,
    }))
    .await
    .expect("probe");
    let reachable: Vec<bool> = result["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| r["reachable"] == true)
        .collect();
    assert_eq!(reachable, vec![true, false, true, false, false]);
    assert_eq!(result["strategy"], "mixed");
    assert_eq!(result["isIntranet"], true);
    assert_eq!(result["results"][2]["marker"], "zcode");
    assert_eq!(
        result["results"][3]["error"],
        "marker mismatch(expected=other, actual=zcode)"
    );
    assert_eq!(result["results"][4]["error"], "service returned ok=false");
    assert_eq!(result["results"][1]["attemptCount"], 1);
    let none = probe_intranet(&json!({"targets": []}))
        .await
        .expect("empty");
    assert!(probe_intranet(&json!({})).await.is_err(), "缺 targets");
    assert_eq!(
        (
            none["isIntranet"].clone(),
            none["requiredSuccessCount"].clone()
        ),
        (json!(false), json!(1))
    );
}
