//! model-IO 记录的调用元数据与偏好（docs/specs/rust-model-io.md）。
//!
//! 模型请求体与重试尝试只在 `HttpModel` 里可见，但 sessionId / turnId / querySource 属于调用方；
//! 用 task-local 作用域从 core 的各调用点传下去，避免为了诊断记录改 `ModelPort::complete` 的签名。
//! 作用域不随 `tokio::spawn` 继承——在新任务里调用模型时要在任务内重新进入作用域。

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct ModelCallScope {
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// TS `metadata.querySource`：`main_turn` / `subagent` / `compact` / `session_title` / …
    pub query_source: Option<String>,
}

tokio::task_local! {
    static MODEL_CALL: ModelCallScope;
}

/// 在 `scope` 下执行一次（或一组）模型调用。
pub async fn with_model_call<F: Future>(scope: ModelCallScope, call: F) -> F::Output {
    MODEL_CALL.scope(scope, call).await
}

/// 继承当前作用域（session / turn），只改 querySource——辅助调用（compact、WebFetch 处理等）用它标注来源。
pub async fn with_query_source<F: Future>(source: &str, call: F) -> F::Output {
    let mut scope = current_model_call();
    scope.query_source = Some(source.to_owned());
    with_model_call(scope, call).await
}

/// 当前任务的调用元数据；不在作用域内时为空（记录落到 `model-io-no-session.jsonl`）。
pub fn current_model_call() -> ModelCallScope {
    MODEL_CALL.try_with(Clone::clone).unwrap_or_default()
}

/// App 全局偏好「完整保留模型 IO」（`workspace/updateModelIoPreferences`）：进程级，对已有与之后的会话
/// 立即生效（TS 同时缓存给未来 session 并更新已有 session 的 adapter）。
static FULL_RETENTION: AtomicBool = AtomicBool::new(false);

pub fn set_model_io_full_retention(enabled: bool) {
    FULL_RETENTION.store(enabled, Ordering::Relaxed);
}

pub fn model_io_full_retention() -> bool {
    FULL_RETENTION.load(Ordering::Relaxed)
}

/// 模型用量事实的落点（TS `usageStore.recordModelUsage`）：每次逻辑请求结束（含重试后的终态）一条。
/// 由进程入口装配到会话库；未装配（测试、工具进程）时丢弃。
pub type ModelUsageSink = Box<dyn Fn(serde_json::Value) + Send + Sync>;
static USAGE_SINK: std::sync::OnceLock<ModelUsageSink> = std::sync::OnceLock::new();

pub fn set_model_usage_sink(sink: ModelUsageSink) {
    let _ = USAGE_SINK.set(sink);
}

pub fn record_model_usage(fact: serde_json::Value) {
    if let Some(sink) = USAGE_SINK.get() {
        sink(fact);
    }
}

/// 工作流 actor 模型请求的准入票据（TS `ModelRequestAdmissionTicket`）：每次尝试的网络状态事件依序投给它，
/// drop 即释放（TS `release()`）。
pub trait ModelAdmissionTicket: Send + Sync {
    fn publish(&self, event: &serde_json::Value);
}
type AdmissionFuture = std::pin::Pin<Box<dyn Future<Output = Option<Box<dyn ModelAdmissionTicket>>> + Send>>;
/// 准入钩子（TS 进程级并发治理器经工作流宿主）：入参是调用作用域与 provider / model。
pub type ModelAdmission = Box<dyn Fn(ModelCallScope, String, String) -> AdmissionFuture + Send + Sync>;
static ADMISSION: std::sync::OnceLock<ModelAdmission> = std::sync::OnceLock::new();

pub fn set_model_admission(admission: ModelAdmission) {
    let _ = ADMISSION.set(admission);
}

/// 只有工作流 actor（`workflow_child`）的请求过闸门（TS 只给 actor runtime 注入准入端口）。
pub async fn acquire_model_admission(
    provider_id: &str,
    model_id: &str,
) -> Option<Box<dyn ModelAdmissionTicket>> {
    let scope = current_model_call();
    if scope.query_source.as_deref() != Some("workflow_child") {
        return None;
    }
    let admission = ADMISSION.get()?;
    admission(scope, provider_id.to_owned(), model_id.to_owned()).await
}
