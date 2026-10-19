//! 实例帧泵（读端唯一消费者）与崩溃回收（docs/specs/rust-u2-host-resilience.md §2.2）。
//! `instance` 的子模块：直接使用父模块的私有状态。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use escode_host_control::logging;
use serde_json::{Value, json};

use super::{ExitReport, InstanceState};
use crate::host_io::{HostError, HostRequest};
use crate::link;

impl InstanceState {
    /// 记录本代的结束（只记第一次：崩溃回收与随后的 shutdown 可能先后到达）。
    pub(super) fn record_exit(&self, report: ExitReport) {
        self.exit.send_if_modified(|slot| {
            if slot.is_some() {
                return false;
            }
            *slot = Some(report);
            true
        });
    }

    /// 崩溃回收：读端意外关闭后立即等实例结束、取残余清单并记 error 日志，不等下一次 `get`
    /// （owner 锁随实例任务结束释放，下一代可以马上拿到；残余任务不拖到下一次请求才中止）。
    async fn reap(&self) {
        let mut lifecycle = self.link_lifecycle.lock().await;
        let (cause, residual) = match lifecycle.wait(self.grace).await {
            Ok(exit) => (exit.cause, exit.residual),
            Err(error) => (format!("{error:#}"), vec![self.event.identity.clone()]),
        };
        drop(lifecycle);
        logging::error(
            "runtime-link",
            format!(
                "runtime instance {} generation {} exited unexpectedly: {cause}; residual: {residual:?}",
                self.event.identity, self.event.generation
            ),
        );
        self.record_exit(ExitReport {
            generation: self.event.generation,
            requested: false,
            cause,
            residual,
        });
    }
}

/// 帧泵：读端唯一消费者。
pub(super) async fn pump(mut reader: link::RuntimeReader, state: Arc<InstanceState>) {
    while let Some(line) = reader.line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(parsed) = serde_json::from_str::<Value>(trimmed).ok() else {
            state.fan_out(trimmed);
            continue;
        };
        if parsed["method"] == "startup/storageState" && parsed["params"]["phase"] == "ready" {
            state.mark_ready();
        }
        let reply_id = parsed["id"].as_str().map(str::to_owned);
        if let Some(waiter) = reply_id
            .as_ref()
            .filter(|_| parsed.get("method").is_none())
            .and_then(|id| state.lock_pending().remove(id))
        {
            let _ = waiter.send(parsed);
            continue;
        }
        if parsed.get("method").is_some_and(Value::is_string) && !parsed["id"].is_null() {
            answer_host_request(&state, parsed);
            continue;
        }
        state.fan_out(trimmed);
    }
    state.close();
    if !state.stopping.load(Ordering::SeqCst) {
        state.reap().await;
    }
}

/// 应答一条反向请求（独立任务：慢应答不阻塞帧泵）。
fn answer_host_request(state: &Arc<InstanceState>, request: Value) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let id = request["id"].clone();
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let outcome = match state.responder.current() {
            Some(responder) => {
                responder(HostRequest {
                    identity: state.event.identity.clone(),
                    workspace_path: state.event.workspace_path.clone(),
                    method: method.clone(),
                    params: request["params"].clone(),
                })
                .await
            }
            None => Err(HostError::unsupported(&method)),
        };
        let response = match outcome {
            Ok(result) => json!({"id": id, "result": result}),
            Err(error) => {
                json!({"id": id, "error": {"code": error.code, "message": error.message}})
            }
        };
        // 发送失败 = 实例已关闭；等待方随实例退出一并失败，这里无需再上报。
        let _ = state.send_line(&response.to_string()).await;
    });
}
