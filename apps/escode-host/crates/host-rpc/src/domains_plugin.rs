//! `plugin-management` channel（docs/specs/rust-u2-plugin-management.md）：参数整形 + 路由 + 超时 + 进度扇出。
//! 插件事实源在 CLI runtime，Host 无副本。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use escode_host_control::logging;
use escode_host_runtime::{Instance, Instances};
use futures_util::stream;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::channel::{Channel, EventHandler, EventStream};
use crate::forward::{self, CallError, RuntimePolicy, RuntimeTarget};
use crate::plugin_params::{CATALOG_METHODS, CATALOG_TIMEOUT, Carrier, SPECS, catalog_body};
use crate::plugin_result_schema::validate;
use crate::plugin_result_tables::{normalize, schema_for};
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `PLUGIN_MANAGEMENT_WORKSPACE_DIR_NAME`。
const MANAGEMENT_DIR: &str = "plugin-workspace";
/// runtime 通知（TS `escodeProtocolNotifications.pluginOperationProgress`）。
const PROGRESS_METHOD: &str = "plugins/operationProgress";

/// 进度扇出：管理面实例的唯一帧订阅者，按 `operationId` 分发。
#[derive(Default)]
struct Hub {
    listeners: Mutex<BTreeMap<String, Vec<mpsc::UnboundedSender<Value>>>>,
    pumped: Mutex<Option<Weak<Instance>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Hub {
    /// 同步登记（早于任何请求），返回该操作的进度流。
    fn listen(&self, operation_id: &str) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        lock(&self.listeners)
            .entry(operation_id.to_owned())
            .or_default()
            .push(tx);
        rx
    }

    fn dispatch(&self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if message["method"] != PROGRESS_METHOD {
            return;
        }
        let params = message["params"].clone();
        let Some(operation_id) = params["operationId"].as_str() else {
            return;
        };
        let mut listeners = lock(&self.listeners);
        if let Some(senders) = listeners.get_mut(operation_id) {
            senders.retain(|tx| tx.send(params.clone()).is_ok());
            if senders.is_empty() {
                listeners.remove(operation_id);
            }
        }
    }

    /// 订阅该实例的出站帧（每个世代只订一次）。必须先于该实例上的任何请求完成。
    async fn attach(self: &Arc<Self>, instance: &Arc<Instance>) {
        {
            let mut pumped = lock(&self.pumped);
            if pumped
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|current| Arc::ptr_eq(&current, instance))
            {
                return;
            }
            *pumped = Some(Arc::downgrade(instance));
        }
        let mut frames = instance.subscribe().await;
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(line) = frames.recv().await {
                hub.dispatch(&line);
            }
        });
    }
}

pub(crate) type PluginAliases = (
    Vec<(&'static str, MethodHandler)>,
    Vec<(&'static str, EventHandler)>,
);

/// 插件管理面：`plugin-management` channel 与 `escode-agent` 旧插件面共用同一实例与进度 Hub。
#[derive(Clone)]
pub(crate) struct PluginManagement {
    instances: Instances,
    options: escode_host_surface::OptionsFactory,
    data_base_dir: Option<PathBuf>,
    hub: Arc<Hub>,
}

impl PluginManagement {
    pub(crate) fn new(
        instances: Instances,
        options: escode_host_surface::OptionsFactory,
        data_base_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            instances,
            options,
            data_base_dir,
            hub: Arc::new(Hub::default()),
        }
    }

    /// 插件管理面实例（两个 channel 共用；懒启动并订阅进度）。
    pub(crate) async fn management_instance(&self) -> Result<Arc<Instance>, RpcError> {
        self.management().await
    }

    /// 管理面 workspace 引用（全局档已保存工作流的兜底载体）。
    pub(crate) fn management_workspace(&self) -> Result<(String, Value), RpcError> {
        let base = self.data_base_dir.as_ref().ok_or_else(|| {
            RpcError::new(
                "NotImplemented",
                "plugin-management needs dataBaseDir: Main must pass dataBaseDir in rpcListen.",
            )
        })?;
        let path = base
            .join(".escode")
            .join(MANAGEMENT_DIR)
            .to_string_lossy()
            .into_owned();
        Ok((
            path.clone(),
            json!({"workspacePath": path, "workspaceKey": path}),
        ))
    }
}

fn params_of(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

impl PluginManagement {
    /// TS `getPluginManagementClient`：固定内部 cwd 的管理面实例（控制面能力不寄居真实 workspace）。
    pub(crate) async fn management(&self) -> Result<Arc<Instance>, RpcError> {
        let base = self.data_base_dir.as_ref().ok_or_else(|| {
            RpcError::new(
                "NotImplemented",
                "plugin-management needs dataBaseDir: Main must pass dataBaseDir in rpcListen.",
            )
        })?;
        let dir = base.join(".escode").join(MANAGEMENT_DIR);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| RpcError::new("Fatal", format!("{}: {error}", dir.display())))?;
        let path = dir.to_string_lossy().into_owned();
        let target = RuntimeTarget {
            identity: path.clone(),
            descriptor: escode_host_control::WorkspaceDescriptor {
                path: path.clone(),
                identity: Some(path),
                data_dir: None,
                config: None,
            },
            request: Value::Null,
        };
        let instance = forward::runtime_instance(
            &self.instances,
            &self.options,
            &target,
            RuntimePolicy::StartIfNeeded,
        )
        .await?;
        // 订阅先于请求：进度通知不会落到订阅之前。
        self.hub.attach(&instance).await;
        Ok(instance)
    }

    /// 回收管理面实例（收口当前世代）；下一次 `management()` 会懒启动新一代并重新 attach。
    async fn recycle_management(&self) {
        let Some(base) = self.data_base_dir.as_ref() else {
            return;
        };
        let identity = base
            .join(".escode")
            .join(MANAGEMENT_DIR)
            .to_string_lossy()
            .into_owned();
        if let Err(error) = self.instances.close(&identity).await {
            logging::warn(
                "plugin-management",
                format!("recycling the management instance failed: {error:#}"),
            );
        }
    }

    async fn carrier(
        &self,
        carrier: Carrier,
        target: &RuntimeTarget,
    ) -> Result<Arc<Instance>, RpcError> {
        match carrier {
            Carrier::Management => self.management().await,
            Carrier::Workspace => {
                forward::runtime_instance(
                    &self.instances,
                    &self.options,
                    target,
                    RuntimePolicy::StartIfNeeded,
                )
                .await
            }
        }
    }
}

/// `{workspace}`：真实 workspace 引用（取自服务级参数，与 `escode-session` 同一构造）。
fn workspace_of(params: &Value) -> Result<Value, RpcError> {
    let target = forward::runtime_target(params.clone())
        .map_err(<RpcError as From<(String, String)>>::from)?;
    Ok(target.request["workspace"].clone())
}

fn method_handler(context: &PluginManagement, index: usize) -> MethodHandler {
    let context = context.clone();
    handler_rpc(move |arg: Wire| {
        let context = context.clone();
        Box::pin(async move {
            let spec = &SPECS[index];
            let params = params_of(arg);
            let workspace = if spec.with_workspace {
                workspace_of(&params)?
            } else {
                Value::Null
            };
            let body = spec.body(&params, &workspace);
            // 路由目标只用于 Workspace 载体；管理面忽略。
            let target = forward::runtime_target(params.clone()).ok();
            let instance = match (spec.carrier, target) {
                (Carrier::Workspace, Some(target)) => {
                    context.carrier(Carrier::Workspace, &target).await?
                }
                _ => context.management().await?,
            };
            if spec.retry_stale {
                return with_stale_retry(
                    spec.runtime,
                    || {
                        let (context, body) = (context.clone(), body.clone());
                        async move {
                            let instance = context.management().await.map_err(CallError::Failed)?;
                            forward::request_classified(
                                &instance,
                                spec.runtime,
                                body,
                                None,
                                spec.timeout,
                            )
                            .await
                        }
                    },
                    || context.recycle_management(),
                )
                .await
                .and_then(|result| check_result(spec.runtime, result))
                .map(Wire::Object);
            }
            forward::request_with_timeout(&instance, spec.runtime, body, None, spec.timeout)
                .await
                .and_then(|result| check_result(spec.runtime, result))
                .map(Wire::Object)
        })
    })
}

/// 已移植 schema 的结果先校验再返回（TS `client.request(…, resultSchema)` 的 `parse`；spec §7）。
/// 校验失败是协议漂移，不是超时：不回收、不重试，直接作为错误交给调用方。
fn check_result(method: &str, mut result: Value) -> Result<Value, RpcError> {
    if let Some(schema) = schema_for(method) {
        validate(schema, &result).map_err(|reason| {
            RpcError::new(
                "Error",
                format!("ESCode Protocol response parse failed: {method}: {reason}"),
            )
        })?;
        normalize(method, &mut result);
    }
    Ok(result)
}

/// 幂等只读请求的 stale 恢复（TS `listPlugins`）：期限内无应答 → 回收实例 → 重试**一次**。
/// 第二次仍失败（含再次超时）原样报错，不无限重试；非超时失败（runtime 的错误应答等）不重试。
pub(crate) async fn with_stale_retry<A, AFut, R, RFut>(
    method: &str,
    mut attempt: A,
    mut recycle: R,
) -> Result<Value, RpcError>
where
    A: FnMut() -> AFut,
    AFut: std::future::Future<Output = Result<Value, CallError>>,
    R: FnMut() -> RFut,
    RFut: std::future::Future<Output = ()>,
{
    match attempt().await {
        Err(CallError::Timeout(error)) => {
            logging::warn(
                "plugin-management",
                format!(
                    "`{method}` timed out, recycling the unresponsive runtime and retrying once: {}",
                    error.message
                ),
            );
            recycle().await;
            attempt().await.map_err(CallError::into_rpc)
        }
        other => other.map_err(CallError::into_rpc),
    }
}

/// `getPluginReferenceCatalog`：打到持有 session 记录的 workspace 实例；新名先试，仅 `-32601` 回退旧名。
fn catalog_handler(context: &PluginManagement) -> MethodHandler {
    let context = context.clone();
    handler_rpc(move |arg: Wire| {
        let context = context.clone();
        Box::pin(async move {
            let params = params_of(arg);
            let target = forward::runtime_target(params.clone())
                .map_err(<RpcError as From<(String, String)>>::from)?;
            let body = catalog_body(&params, &target.request["workspace"]);
            let instance = context.carrier(Carrier::Workspace, &target).await?;
            let first = forward::request_with_timeout(
                &instance,
                CATALOG_METHODS.0,
                body.clone(),
                None,
                CATALOG_TIMEOUT,
            )
            .await;
            match first {
                Err(error) if error.extra.get("code") == Some(&Value::from(-32601)) => {
                    forward::request_with_timeout(
                        &instance,
                        CATALOG_METHODS.1,
                        body,
                        None,
                        CATALOG_TIMEOUT,
                    )
                    .await
                    .and_then(|result| check_result(CATALOG_METHODS.1, result))
                    .map(Wire::Object)
                }
                // 新方法名的结果校验失败是协议漂移，不是“方法不存在”：不回退旧名（TS 同：ZodError 没有 code）。
                other => other
                    .and_then(|result| check_result(CATALOG_METHODS.0, result))
                    .map(Wire::Object),
            }
        })
    })
}

fn progress_event(context: &PluginManagement) -> EventHandler {
    let hub = Arc::clone(&context.hub);
    Arc::new(move |arg: Wire| {
        let operation_id = match arg.into_args().into_iter().next() {
            Some(Wire::Str(id)) => id,
            _ => String::new(),
        };
        let receiver = hub.listen(&operation_id);
        let stream: EventStream = Box::pin(stream::unfold(receiver, |mut receiver| async move {
            receiver
                .recv()
                .await
                .map(|value| (Wire::Object(value), receiver))
        }));
        Ok(stream)
    })
}

impl PluginManagement {
    /// `plugin-management` channel（方法表 + 进度事件）。
    pub(crate) fn channel(&self) -> Channel {
        let (methods, events) = self.aliases();
        Channel::new("plugin-management", methods).with_events(events)
    }

    /// `escode-agent` 旧插件面的别名方法/事件。TS 这两个面共用同一个
    /// `getPluginManagementClient()`，因此直接复用同一套处理函数与进度 Hub。
    pub(crate) fn aliases(&self) -> PluginAliases {
        let mut methods: Vec<(&'static str, MethodHandler)> = SPECS
            .iter()
            .enumerate()
            .map(|(index, spec)| (spec.service, method_handler(self, index)))
            .collect();
        methods.push(("getPluginReferenceCatalog", catalog_handler(self)));
        (
            methods,
            vec![("onDynamicPluginOperationProgress", progress_event(self))],
        )
    }
}

#[cfg(test)]
#[path = "domains_plugin_tests.rs"]
mod tests;
