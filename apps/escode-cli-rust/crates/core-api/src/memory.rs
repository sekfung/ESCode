//! 项目记忆的跨层数据（docs/specs/rust-project-memory.md）。
use serde_json::Value;
use std::sync::Arc;

/// 会话启用记忆时的根目录与 MEMORY.md 原文（首轮前读取，会话内固定，与 TS 上下文初始化一致）。
#[derive(Clone, Debug, Default)]
pub struct ProjectMemory {
    pub root: String,
    pub index_path: String,
    pub index: Option<String>,
}

/// 主轮次成功完成时的提取快照：provider 消息（系统前缀 + 投影历史）、工具目录与本轮模型。
pub struct MemorySnapshot {
    pub memory: ProjectMemory,
    pub messages: Vec<Value>,
    pub definitions: Vec<Value>,
    pub model: Arc<dyn super::ModelPort>,
}
