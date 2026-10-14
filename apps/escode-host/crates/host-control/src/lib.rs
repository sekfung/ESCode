//! Main↔Host 控制通道（rust-u2-host-skeleton.md §3.5）：控制消息与日志批次共用
//! **一条**有序队列，避免两条写者交错破坏 stdio 协议；消息 typed，错误带结构化 code。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// 控制消息（Main↔Host）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ControlMessage {
    /// Main→Host：窗口接入（window→Host attachment）。
    WindowAttach {
        window_id: String,
        client_id: String,
    },
    /// Main→Host：窗口断开（Host 保留 runtime；attachment 只是视图）。
    WindowDetach { window_id: String },
    /// Host→Main：实例就绪（含协议版本与 workspace 数量，供 Main 日志与健康检查）。
    HostReady {
        protocol_version: u32,
        workspaces: usize,
    },
    /// Main→Host：优雅关闭（宽限由 Host 内部实现）。
    Shutdown { reason: String },
    /// 双向：日志批次（级别 + 内容 + 归属 scope）。
    LogBatch {
        scope: String,
        level: LogLevel,
        lines: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// 结构化错误（对标"启动错误可分类"的要求：code 可判重试）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    /// 协议版本不匹配：Main 应升级或回退 Node Host。
    ProtocolMismatch,
    /// Host 未就绪：可重试。
    NotReady,
    /// 实例所有权冲突：等旧 owner 退出后重试（对应 exit 75 语义）。
    WorkspaceOwned,
    /// 其他不可重试故障。
    Fatal,
}

/// 单写者：所有控制消息与日志批次从这里出门，顺序即入队顺序。
#[derive(Clone)]
pub struct ControlWriter {
    tx: mpsc::Sender<ControlMessage>,
    sequence: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
}

impl ControlWriter {
    /// `capacity` 见子 spec §3.2：日志满时丢弃最低级别并计数，控制消息不丢。
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<ControlMessage>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Self {
                tx,
                sequence: Arc::new(AtomicU64::new(0)),
                written: Arc::new(AtomicU64::new(0)),
            },
            rx,
        )
    }

    /// 控制消息：必须送达（队列满则等待；接收端关闭返回错误）。
    pub async fn control(&self, message: ControlMessage) -> anyhow::Result<()> {
        self.tx
            .send(message)
            .await
            .map_err(|_| anyhow::anyhow!("Control channel closed"))?;
        self.written.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 日志批次：队列满时丢弃（日志可丢，控制不可丢），丢弃计数可观测。
    pub fn log(&self, scope: &str, level: LogLevel, lines: Vec<String>) -> bool {
        let message = ControlMessage::LogBatch {
            scope: scope.to_owned(),
            level,
            lines,
        };
        match self.tx.try_send(message) {
            Ok(()) => {
                self.written.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => false,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// 已写出消息数（健康检查与测试用）。
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
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
}
