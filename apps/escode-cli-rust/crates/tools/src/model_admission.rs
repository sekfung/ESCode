//! 工作流 actor 模型请求的准入（docs/specs/rust-dynamic-workflow.md）：TS 进程级并发治理器
//! （`getWorkflowConcurrencyGovernor`）住在工作流宿主；actor 的每次尝试先向宿主取票，状态事件依序投给票据，
//! drop 即释放。宿主不可用时放行（不设闸门，与 TS 端口缺席一致）。
use super::WorkspaceTools;
use crate::contract::{ModelAdmissionTicket, ModelCallScope};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;

/// 票据的发布与释放走同一条有序通道（TS 治理器按事件顺序结算：started → failed → retry_scheduled → release）。
struct HostTicket {
    id: String,
    tx: mpsc::UnboundedSender<(&'static str, Value)>,
}

impl ModelAdmissionTicket for HostTicket {
    fn publish(&self, event: &Value) {
        let _ = self.tx.send((
            "actor.admission.publish",
            json!({ "ticket": self.id, "event": event }),
        ));
    }
}

impl Drop for HostTicket {
    fn drop(&mut self) {
        let _ = self
            .tx
            .send(("actor.admission.release", json!({ "ticket": self.id })));
    }
}

impl WorkspaceTools {
    /// 进程入口装配一次：之后 `workflow_child` 的模型尝试都经宿主准入。
    pub fn with_model_admission(self) -> Self {
        // 全局钩子只持弱引用：不能让进程级静态量把宿主（及其子进程）留到退出之后。
        let host = Arc::downgrade(&self.workflow_host);
        crate::contract::set_model_admission(Box::new(
            move |scope: ModelCallScope, provider, model| {
                let host = host.upgrade();
                Box::pin(async move {
                    let host = host?;
                    let params = json!({ "actorSession": scope.session_id, "providerId": provider, "modelId": model });
                    let reply = host.request("actor.admission.acquire", params).await.ok()?;
                    let id = reply["ticket"].as_str()?.to_owned();
                    let (tx, mut rx) = mpsc::unbounded_channel::<(&'static str, Value)>();
                    let ordered: Arc<_> = host.clone();
                    tokio::spawn(async move {
                        while let Some((method, params)) = rx.recv().await {
                            let _ = ordered.request(method, params).await;
                        }
                    });
                    Some(Box::new(HostTicket { id, tx }) as Box<dyn ModelAdmissionTicket>)
                })
            },
        ));
        self
    }
}
