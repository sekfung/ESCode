//! 慢插件操作（安装、更新、市场增删刷新）作为后台作业执行（docs/specs/rust-plugin-marketplace-write.md W5）：
//! 与 TS 一样不阻塞其它请求，并可按 `operationId` 取消（TS `withPluginOperationSignal` / `cancelPluginOperation`）。
//! 取消只置位令牌，由操作在安全点检查（提交点之后不再响应），不丢弃进行中的 future。
use super::Engine;
use super::auxiliary::Auxiliary;
use crate::contract::{Event, EventSink};
use crate::domain::protocol::Request;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// 走后台作业的插件方法。
pub(super) const PLUGIN_JOB_METHODS: &[&str] = &[
    "plugins/install",
    "plugins/update",
    "plugins/marketplace/add",
    "plugins/marketplace/update",
    "plugins/validate",
    "plugins/describe",
];

impl Engine {
    pub(super) fn start_plugin_job(&mut self, request: &Request) -> Result<()> {
        self.validate_workspace(&request.params)?;
        let operation = request.params["operationId"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let id = format!("plugin-op:{}", self.clock.id());
        let cancel = CancellationToken::new();
        self.auxiliary.insert(
            id.clone(),
            Auxiliary {
                request: request.id.clone(),
                cancel: cancel.clone(),
                operation,
                session: None,
            },
        );
        let sink = EventSink {
            session_id: id.clone(),
            run_id: id,
            tx: self.events.clone(),
        };
        let tools = self.tools.clone();
        let (method, params) = (request.method.clone(), request.params.clone());
        tokio::spawn(async move {
            let result = tools
                .plugin_operation(&method, &params, &cancel)
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = sink.send(Event::AuxiliaryReply { result }).await;
        });
        Ok(())
    }

    /// TS `cancelPluginOperation`：找到同 operationId 的进行中作业即取消（之后同 id 再取消返回 false）。
    pub(super) fn cancel_plugin_operation(&mut self, p: &Value) -> Result<Value> {
        let operation = p["operationId"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("Invalid params — operationId must be a non-empty string")?
            .to_owned();
        let job = self
            .auxiliary
            .values_mut()
            .find(|job| job.operation.as_deref() == Some(operation.as_str()));
        let cancelled = match job {
            Some(job) => {
                job.cancel.cancel();
                job.operation = None;
                true
            }
            None => false,
        };
        Ok(json!({ "operationId": operation, "cancelled": cancelled }))
    }
}
