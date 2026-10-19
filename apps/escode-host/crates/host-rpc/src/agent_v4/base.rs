//! V4 面的 runtime 转发层（TS `escodeAgentService.ts:5850-6600` 的 base service）。
//!
//! 每个方法的请求体逐字对齐 TS `client.request(V4_METHODS.x, {...})` 字面量；可信连接字段
//! （`connectionId`/`clientMode`/`workflowRunDeltas`）只由连接作用域经 [`Connection`] 注入，
//! Renderer 入参里的同名字段从不进入请求体。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value, json};

use super::frames::FrameHub;
use crate::domains_task_support::ReadContext;
use crate::forward::{self, RuntimePolicy};
use crate::session::{ClientMode, RpcError};

/// TS `PROTOCOL_V4_LIMITS.maxFrameBytes`。
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 一条连接的可信身份（TS `ESCodeAgentV4ConnectionContext`）。
#[derive(Clone, Debug)]
pub(crate) struct Connection {
    pub connection_id: String,
    pub client_mode: ClientMode,
    pub workflow_run_deltas: bool,
}

/// Host 级共享句柄（跨连接）。
pub(crate) struct Base {
    pub instances: escode_host_runtime::Instances,
    pub options: escode_host_surface::OptionsFactory,
    pub hub: Arc<FrameHub>,
    /// 冷订阅的 `resumeThoughtLevel` hint 来源（TS `automationTaskIndexRepo.getTaskMeta`）。
    pub index: Option<ReadContext>,
    /// `runtime/capabilities` 的 `independentPlanState`，按 (身份键, 代次) 缓存（TS 按 client 缓存）。
    plan_support: Mutex<BTreeMap<(String, u64), bool>>,
    /// provider runtime headers 反向请求注册表（与 runtime responder 共用）。
    pub provider_headers: Option<Arc<crate::provider_headers::ProviderHeaders>>,
}

/// 一次 runtime 调用的结果。
pub(crate) struct Called {
    pub result: Value,
}

impl Base {
    pub(crate) fn new(
        instances: escode_host_runtime::Instances,
        options: escode_host_surface::OptionsFactory,
        index: Option<ReadContext>,
        provider_headers: Option<Arc<crate::provider_headers::ProviderHeaders>>,
    ) -> Self {
        Self {
            instances,
            options,
            hub: Arc::default(),
            index,
            plan_support: Mutex::default(),
            provider_headers,
        }
    }

    /// 按策略取实例并发一条请求。
    pub(crate) async fn call(
        &self,
        target: &Value,
        policy: RuntimePolicy,
        method: &str,
        body: Value,
    ) -> Result<Called, RpcError> {
        let routing = forward::runtime_target(target.clone())?;
        let instance =
            forward::runtime_instance(&self.instances, &self.options, &routing, policy).await?;
        let result = forward::request(&instance, method, body, None).await?;
        Ok(Called { result })
    }

    /// 订阅（三类 topic 同一个 RPC）：先接帧泵再发请求，保证 ACK 前到达的帧可被暂存。
    pub(crate) async fn subscribe(
        &self,
        target: &Value,
        policy: RuntimePolicy,
        body: Value,
        started: Instant,
    ) -> Result<Subscribed, RpcError> {
        let routing = forward::runtime_target(target.clone())?;
        let reused = self.instances.live(&routing.identity).is_some();
        let bootstrap = Instant::now();
        let instance =
            forward::runtime_instance(&self.instances, &self.options, &routing, policy).await?;
        let bootstrap_ms = millis(bootstrap);
        self.hub.ensure_pump(&instance).await;
        let request_started = Instant::now();
        let result = forward::request(&instance, "v4/conversation/subscribe", body, None).await?;
        Ok(Subscribed {
            result,
            timing: Timing {
                // TS `cliRequestStartedAt - subscribeStartedAt`（含索引 hint 读取与实例准备）。
                host_prepare_ms: u64::try_from(request_started.duration_since(started).as_millis())
                    .unwrap_or(u64::MAX),
                cli_bootstrap_ms: (!reused).then_some(bootstrap_ms),
                reused,
                cli_request_ms: millis(request_started),
            },
        })
    }

    /// `ensureIndependentPlanSupport`：发 `planEnabled` 命令前确认执行端支持独立计划态。
    pub(crate) async fn ensure_plan_support(&self, target: &Value) -> Result<(), RpcError> {
        let routing = forward::runtime_target(target.clone())?;
        let instance = forward::runtime_instance(
            &self.instances,
            &self.options,
            &routing,
            RuntimePolicy::StartIfNeeded,
        )
        .await?;
        let key = (instance.identity.clone(), instance.generation);
        if let Some(supported) = lock(&self.plan_support).get(&key).copied() {
            return plan_result(supported);
        }
        let supported =
            match forward::request(&instance, "runtime/capabilities", json!({}), None).await {
                Ok(capabilities) => capabilities["independentPlanState"] == true,
                // TS：`-32601` 视为空能力（不支持）；其余错误不缓存，原样失败为 unsupported。
                Err(error) if error.extra.get("code") == Some(&Value::from(-32601)) => false,
                Err(_) => return plan_result(false),
            };
        lock(&self.plan_support).insert(key, supported);
        plan_result(supported)
    }

    /// 冷订阅的 thoughtLevel hint（索引不可用即省略，TS 同为失败即无 hint）。
    pub(crate) async fn resume_thought_level(&self, target: &Value) -> Option<String> {
        let index = self.index.as_ref()?;
        let workspace_path = target["workspacePath"].as_str()?.to_owned();
        let workspace_identity = target["workspaceIdentity"].as_str().map(str::to_owned);
        let task_id = target["sessionId"].as_str()?.to_owned();
        let index = crate::domains_task_support::context_ref(index);
        index
            .run(move |index| {
                index
                    .get_task_meta(&workspace_path, workspace_identity.as_deref(), &task_id)
                    .map_err(|error| error.to_string())
            })
            .await
            .ok()
            .flatten()
            .and_then(|meta| {
                meta["thoughtLevel"]
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
    }
}

/// 订阅应答 + 计时（TS ACK 上的 `openTiming` 来源）。
pub(crate) struct Subscribed {
    pub result: Value,
    pub timing: Timing,
}

pub(crate) struct Timing {
    pub host_prepare_ms: u64,
    pub cli_bootstrap_ms: Option<u64>,
    pub reused: bool,
    pub cli_request_ms: u64,
}

/// TS `subscribeConversationV4` 返回前在 ACK 上补的 `openTiming`（诊断字段，版本 1）。
pub(crate) fn with_open_timing(
    mut result: Value,
    timing: &Timing,
    task_meta_read_ms: u64,
) -> Value {
    let mut open = result["ack"]["openTiming"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    open.insert("version".to_owned(), json!(1));
    open.insert("hostPrepareMs".to_owned(), json!(timing.host_prepare_ms));
    if let Some(bootstrap) = timing.cli_bootstrap_ms {
        open.insert("cliBootstrapMs".to_owned(), json!(bootstrap));
    }
    open.insert(
        "cliProcessState".to_owned(),
        json!(if timing.reused { "reused" } else { "spawned" }),
    );
    // 账号 provider 同步屏障未迁移（登记差异 §6-3）：同步耗时恒为 0。
    open.insert("providerRegistrySyncMs".to_owned(), json!(0));
    open.insert("taskMetaReadMs".to_owned(), json!(task_meta_read_ms));
    open.insert("cliRequestMs".to_owned(), json!(timing.cli_request_ms));
    if let Some(ack) = result["ack"].as_object_mut() {
        ack.insert("openTiming".to_owned(), Value::Object(open));
    }
    result
}

fn plan_result(supported: bool) -> Result<(), RpcError> {
    if supported {
        Ok(())
    } else {
        Err(RpcError::new("Error", "proto.independentPlanUnsupported"))
    }
}

fn millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ── 请求体（TS 字面量；缺省/`undefined` 字段整键省略） ──

/// 只拷贝入参里存在且非 null 的键。
pub(crate) fn pick(params: &Value, keys: &[&str]) -> Map<String, Value> {
    let mut body = Map::new();
    for key in keys {
        if let Some(value) = params.get(*key).filter(|value| !value.is_null()) {
            body.insert((*key).to_owned(), value.clone());
        }
    }
    body
}

/// conversation 订阅请求体（TS `subscribeConversationV4`）。
pub(crate) fn conversation_subscribe_body(
    params: &Value,
    topic: &str,
    connection: &Connection,
    workspace: Value,
    resume_thought_level: Option<String>,
) -> Value {
    let mut body = Map::new();
    body.insert("topic".to_owned(), Value::from(topic));
    body.insert(
        "connectionId".to_owned(),
        Value::from(connection.connection_id.clone()),
    );
    body.insert(
        "clientMode".to_owned(),
        Value::from(connection.client_mode.as_str()),
    );
    if connection.workflow_run_deltas {
        body.insert("workflowRunDeltas".to_owned(), Value::Bool(true));
    }
    body.insert("workspace".to_owned(), workspace);
    if let Some(level) = resume_thought_level {
        body.insert("resumeThoughtLevel".to_owned(), Value::from(level));
    }
    body.extend(truthy(params, &["base", "visibility"]));
    Value::Object(body)
}

/// sessions-index / workspace-config 订阅请求体。
pub(crate) fn index_subscribe_body(params: &Value, topic: &str, connection: &Connection) -> Value {
    let mut body = Map::new();
    body.insert("topic".to_owned(), Value::from(topic));
    body.insert(
        "connectionId".to_owned(),
        Value::from(connection.connection_id.clone()),
    );
    body.insert(
        "clientMode".to_owned(),
        Value::from(connection.client_mode.as_str()),
    );
    body.extend(truthy(params, &["base", "visibility"]));
    Value::Object(body)
}

/// TS `...(params.x ? { x } : {})`：只在真值时带。
fn truthy(params: &Value, keys: &[&str]) -> Map<String, Value> {
    let mut body = Map::new();
    for key in keys {
        if let Some(value) = params.get(*key).filter(|value| is_truthy(value)) {
            body.insert((*key).to_owned(), value.clone());
        }
    }
    body
}

pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `assertV4AttachmentNdjsonEnvelope`：用 32 字符 id 做保守的整帧字节估计（+1 换行）。
pub(crate) fn assert_attachment_envelope(method: &str, params: &Value) -> Result<(), RpcError> {
    let envelope = json!({"id": "9".repeat(32), "method": method, "params": params});
    let bytes = serde_json::to_vec(&envelope).map_or(usize::MAX, |bytes| bytes.len()) + 1;
    if bytes > MAX_FRAME_BYTES {
        return Err(RpcError::new("Error", "proto.frameTooLarge"));
    }
    Ok(())
}
