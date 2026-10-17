//! `ControlWriter` 单队列与错误码单测（docs/specs/rust-u2-host-skeleton.md §3.2/§3.5）。
use super::*;

#[tokio::test]
async fn control_and_logs_share_one_ordered_queue() {
    let (writer, mut rx) = ControlWriter::channel(8);
    writer
        .control(ControlMessage::HostReady {
            protocol_version: 1,
            workspaces: 2,
        })
        .await
        .unwrap();
    assert!(writer.log("settings", LogLevel::Info, vec!["written".into()]));
    writer
        .control(ControlMessage::Shutdown {
            reason: "main exiting".into(),
        })
        .await
        .unwrap();

    // 顺序 = 入队顺序（日志不插队控制，控制不越过日志）。
    assert_eq!(
        rx.recv().await,
        Some(ControlMessage::HostReady {
            protocol_version: 1,
            workspaces: 2
        })
    );
    assert_eq!(
        rx.recv().await,
        Some(ControlMessage::LogBatch {
            scope: "settings".into(),
            level: LogLevel::Info,
            lines: vec!["written".into()]
        })
    );
    assert_eq!(
        rx.recv().await,
        Some(ControlMessage::Shutdown {
            reason: "main exiting".into()
        })
    );
    assert_eq!(writer.written(), 3);
}

#[tokio::test]
async fn logs_drop_when_full_but_control_waits() {
    let (writer, mut rx) = ControlWriter::channel(1);
    assert!(writer.log("a", LogLevel::Debug, vec!["1".into()]));
    assert!(
        !writer.log("a", LogLevel::Debug, vec!["2".into()]),
        "日志满即丢弃"
    );
    let pending = tokio::spawn({
        let writer = writer.clone();
        async move {
            writer
                .control(ControlMessage::Shutdown {
                    reason: "drain".into(),
                })
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished(), "控制消息在队满时等待而不丢");
    let _ = rx.recv().await;
    pending.await.unwrap().unwrap();
    assert!(matches!(
        rx.recv().await,
        Some(ControlMessage::Shutdown { .. })
    ));
}

/// 队满时 warn / error 也被丢弃并计数（spec §3.2 取「任何级别都可能丢，并计数」）；
/// 控制消息不受影响，腾出空间后日志恢复投递。
#[tokio::test]
async fn full_queue_drops_logs_of_every_level_and_counts_them() {
    let (writer, mut rx) = ControlWriter::channel(1);
    assert!(writer.log("a", LogLevel::Info, vec!["fills".into()]));
    assert_eq!(writer.dropped_logs(), 0);
    for level in [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ] {
        assert!(
            !writer.log("a", level, vec!["dropped".into()]),
            "{level:?} 在队满时也被丢弃"
        );
    }
    assert_eq!(writer.dropped_logs(), 4);
    assert_eq!(writer.written(), 1);
    // 克隆共享同一计数（sink 持有的是克隆）。
    let clone = writer.clone();
    assert!(!clone.log("a", LogLevel::Error, vec!["dropped".into()]));
    assert_eq!(writer.dropped_logs(), 5);

    let _ = rx.recv().await;
    assert!(writer.log("a", LogLevel::Warn, vec!["after".into()]));
    assert_eq!(writer.dropped_logs(), 5);
    drop(rx);
    assert!(
        !writer.log("a", LogLevel::Error, vec!["closed".into()]),
        "接收端关闭后同样计入丢弃"
    );
    assert_eq!(writer.dropped_logs(), 6);
}

#[tokio::test]
async fn errors_are_classified_for_retry_decisions() {
    let error = ControlError {
        code: ControlErrorCode::WorkspaceOwned,
        message: "another instance owns the workspace".into(),
    };
    let json = serde_json::to_value(&error).unwrap();
    assert_eq!(json["code"], "workspace_owned");
    let decoded: ControlError = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, error);
}

#[test]
fn rpc_listen_carries_main_composed_runtime_env_in_camel_case() {
    let line = r#"{"kind":"rpcListen","bind":"127.0.0.1:0","home":"/h","dataBaseDir":"/d","runtimeEnv":{"A":"1"}}"#;
    let message: ControlMessage = serde_json::from_str(line).unwrap();
    let ControlMessage::RpcListen { runtime_env, .. } = &message else {
        panic!("unexpected message {message:?}");
    };
    assert_eq!(runtime_env.as_ref().unwrap()["A"], "1");
    // 缺省（旧版 Main）仍可解析；回写不输出空字段。
    let legacy: ControlMessage =
        serde_json::from_str(r#"{"kind":"rpcListen","bind":"b","home":"/h"}"#).unwrap();
    let json = serde_json::to_value(&legacy).unwrap();
    assert!(json.get("runtimeEnv").is_none());
}

#[test]
fn local_media_preview_frames_round_trip_and_the_capability_defaults_off() {
    let listen: ControlMessage =
        serde_json::from_str(r#"{"kind":"rpcListen","bind":"127.0.0.1:0","home":"/h"}"#).unwrap();
    assert!(matches!(
        listen,
        ControlMessage::RpcListen {
            local_media_preview: false,
            ..
        }
    ));
    let declared: ControlMessage = serde_json::from_str(
        r#"{"kind":"rpcListen","bind":"127.0.0.1:0","home":"/h","localMediaPreview":true}"#,
    )
    .unwrap();
    assert!(matches!(
        declared,
        ControlMessage::RpcListen {
            local_media_preview: true,
            ..
        }
    ));
    let request = ControlMessage::LocalMediaPreviewAuthorize {
        request_id: "r1".into(),
        path: "/a b.mp4".into(),
    };
    assert_eq!(
        serde_json::to_string(&request).unwrap(),
        r#"{"kind":"localMediaPreviewAuthorize","requestId":"r1","path":"/a b.mp4"}"#
    );
    let denied: ControlMessage = serde_json::from_str(
        r#"{"kind":"localMediaPreviewAuthorized","requestId":"r1","ok":false,"error":"no"}"#,
    )
    .unwrap();
    assert_eq!(
        denied,
        ControlMessage::LocalMediaPreviewAuthorized {
            request_id: "r1".into(),
            ok: false,
            path: None,
            error: Some("no".into()),
        }
    );
}
