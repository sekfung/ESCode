//! LLM Wiki 端口（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §9–§11）。
//! 适配器（escode-cli-wiki）在组合根按 workspace 创建并绑定项目身份；core 只经本端口访问知识，
//! 不持有知识事实。所有方法都可能失败：自动检索由 core fail-open，工具与 RPC 把错误原样返回。
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

/// run 开始时的 Wiki 可见性：是否注册工具、是否每轮自动注入。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WikiSurface {
    pub enabled: bool,
    pub tools: bool,
    pub auto_inject: bool,
}

pub struct WikiTurnInput {
    pub session_id: String,
    /// 用户本轮真实输入（不含展开后的命令模板）。
    pub prompt: String,
    /// 上下文中已注入的条目版本（id → updated_at），同版本不重复注入。
    pub already_injected: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WikiInjected {
    pub id: String,
    pub version: String,
    pub tokens: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WikiTurnRetrieval {
    /// injected | no_match | empty | disabled
    pub status: &'static str,
    pub workflow: &'static str,
    /// 隐藏上下文正文（未包裹 `<system-reminder>`）；仅 status=injected 时存在。
    pub body: Option<String>,
    pub injected: Vec<WikiInjected>,
    /// 检索命中数（含未注入），与 injected 分开：区分「出现在结果中」与「送入上下文」。
    pub retrieved: usize,
    pub filtered: usize,
    pub duration_ms: u64,
    pub index_versions: String,
}

/// 工具执行结果：模型可见文本 + 结构化结果；`failed` 表示业务失败（仍回给模型）。
#[derive(Clone, Debug, PartialEq)]
pub struct WikiToolResult {
    pub content: String,
    pub structured: Value,
    pub failed: bool,
}

#[async_trait]
pub trait WikiPort: Send + Sync {
    async fn surface(&self) -> WikiSurface;
    async fn retrieve_for_turn(
        &self,
        input: WikiTurnInput,
        cancel: &CancellationToken,
    ) -> Result<WikiTurnRetrieval>;
    async fn tool(&self, name: &str, input: &Value, session_id: &str) -> Result<WikiToolResult>;
    /// Desktop 协议请求（`v4/wiki/status|graph|entry|manage`）。
    async fn request(&self, method: &str, params: &Value) -> Result<Value>;
}
