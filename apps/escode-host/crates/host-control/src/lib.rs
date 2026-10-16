//! Main↔Host 控制通道（rust-u2-host-skeleton.md §3.5）：控制消息与日志批次共用
//! **一条**有序队列，避免两条写者交错破坏 stdio 协议；消息 typed，错误带结构化 code。

pub mod logging;
pub mod path_authorizer;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// 一个 workspace 的装配描述（Main 提交；Host 据此建实例，业务事实不从 Host env 推断）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDescriptor {
    /// Host 提交的 workspace 路径（不 realpath，提示词与身份用它）。
    pub path: String,
    /// 远程/显式身份（缺省按路径，AGENTS.md Workspace Identity）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
}

/// 控制消息（Main↔Host）。方向按变体语义：`WindowRequest` 为 Main→Host，
/// `WindowAttached` / `WindowFrame` 为 Host→Main（u2a 的 Host 内协议；Renderer 直连的
/// WebSocket surface 在其上叠加，见 u2 子 spec §3.4）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ControlMessage {
    /// Main→Host：窗口接入（window→Host attachment）。
    WindowAttach {
        window_id: String,
        client_id: String,
        workspace: WorkspaceDescriptor,
    },
    /// Host→Main：接入完成（含该 workspace 的实例代次）。
    WindowAttached { window_id: String, generation: u64 },
    /// Main→Host：转发一条 Renderer 请求行（NDJSON）。
    WindowRequest { window_id: String, line: String },
    /// Host→Main：转发一条 runtime 出站帧（NDJSON）。
    WindowFrame { window_id: String, line: String },
    /// Main→Host：窗口断开（Host 保留 runtime；attachment 只是视图）。
    WindowDetach { window_id: String },
    /// Host→Main：窗口断开完成（若该实例已无 attachment，代次仍在，仅记录）。
    WindowDetached { window_id: String },
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
    /// Host→Main：请求失败（结构化 code，Main 据此决定重试或回退 Node Host）。
    Failed { error: ControlError },
    /// Host→Main：收口完成（残余任务/进程清单；空 = 干净退出）。
    Stopped { residual: Vec<String> },
    /// Main→Host：读取全局设置（settings 域；home = 用户主目录）。
    SettingsRead { home: String },
    /// Main→Host：应用设置补丁（顶层合并，保留其它键）。
    SettingsWrite {
        home: String,
        patch: serde_json::Value,
    },
    /// Host→Main：设置内容（读应答与写回执共用）。
    SettingsValue { settings: serde_json::Value },
    /// Main→Host：请求开启 Renderer 传输面（WebSocket 监听）。
    SurfaceListen { bind: String },
    /// Host→Main：传输面已监听（Main 据此把端口交给 Renderer）。
    SurfaceListening { addr: String },
    /// Main→Host：请求开启 Renderer RPC 面（`@escode/rpc` 协议跑在 WebSocket 上）。
    /// `home` 是设置域根目录（Main 决定，Host 不读进程 env 推断业务事实）。
    /// `dataBaseDir` 是应用数据根（TS `getDataBaseDir()`：设置项 `dataBaseDir` >
    /// `ESCODE_DATA_BASE_DIR` > HOME），供会话工作区等路径派生；缺省时相关方法回显式缺口。
    RpcListen {
        bind: String,
        home: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data_base_dir: Option<String>,
        /// Main 为嵌入运行时合成的环境变量（TS Host 拉起 CLI 时注入的那一组：provider 配置文件路径、
        /// 数据/日志根、凭据密钥、插件宿主入口……）。Host 不推断这些业务事实，只原样交给每个实例。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_env: Option<std::collections::BTreeMap<String, String>>,
        /// Main 声明会应答 `localMediaPreviewAuthorize`（rust-u2-attachments.md §5）；缺省 false =
        /// 旧 Main，`media-preview.prepare` 只走内联分支。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        local_media_preview: bool,
    },
    /// Host→Main：RPC 面已监听。`secret` 是本次监听的进程密钥（只走控制通道）：Main 带它向
    /// `POST /api/rpc-host-capability` 申请一次性 capability，再连 `ws://addr/ws/host`。
    RpcListening { addr: String, secret: String },
    /// Host→Main：请求授权一个本地媒体路径（Main 的授权注册表是唯一所有者）。
    LocalMediaPreviewAuthorize { request_id: String, path: String },
    /// Main→Host：授权结果（`ok` 时 `path` 为规范路径；否则 `error` 为原文错误）。
    LocalMediaPreviewAuthorized {
        request_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

impl ControlMessage {
    /// 线上的 kind 名（serde tag；契约清单对账用，必须与 `host-proto` 清单一致）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::WindowAttach { .. } => "windowAttach",
            Self::WindowAttached { .. } => "windowAttached",
            Self::WindowRequest { .. } => "windowRequest",
            Self::WindowFrame { .. } => "windowFrame",
            Self::WindowDetach { .. } => "windowDetach",
            Self::WindowDetached { .. } => "windowDetached",
            Self::HostReady { .. } => "hostReady",
            Self::Shutdown { .. } => "shutdown",
            Self::LogBatch { .. } => "logBatch",
            Self::Failed { .. } => "failed",
            Self::Stopped { .. } => "stopped",
            Self::SettingsRead { .. } => "settingsRead",
            Self::SettingsWrite { .. } => "settingsWrite",
            Self::SettingsValue { .. } => "settingsValue",
            Self::SurfaceListen { .. } => "surfaceListen",
            Self::SurfaceListening { .. } => "surfaceListening",
            Self::RpcListen { .. } => "rpcListen",
            Self::RpcListening { .. } => "rpcListening",
            Self::LocalMediaPreviewAuthorize { .. } => "localMediaPreviewAuthorize",
            Self::LocalMediaPreviewAuthorized { .. } => "localMediaPreviewAuthorized",
        }
    }
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
            serde_json::from_str(r#"{"kind":"rpcListen","bind":"127.0.0.1:0","home":"/h"}"#)
                .unwrap();
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
}
