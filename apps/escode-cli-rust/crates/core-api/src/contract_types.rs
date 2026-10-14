//! 运行时端口共享的小类型（从 contract.rs 拆出，经 `contract` 原路径转发保持不变）。

use serde_json::Value;
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub provider_id: String,
    pub model_id: String,
    pub reasoning_level: String,
}

pub type Output = mpsc::Sender<Vec<Value>>;

pub struct ChildHandle {
    pub task: escode_cli_domain::subagent::Task,
    pub updates: tokio::sync::watch::Receiver<escode_cli_domain::subagent::Task>,
    pub message_id: Option<String>,
    pub delivery: Option<String>,
}
