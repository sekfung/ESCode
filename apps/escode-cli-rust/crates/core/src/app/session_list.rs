use super::Engine;
use crate::domain::session_listing::{ListParams, MAX_LIST_BYTES};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::collections::HashSet;

/// TS `TASK_LIST_SESSION_TYPES`。
const LIST_TYPES: [&str; 3] = ["interactive", "fork", "workflow_parent"];

impl Engine {
    pub(super) async fn list_sessions(&self, p: &Value) -> Result<Value> {
        let params = ListParams::parse(p)?;
        let records = self
            .store
            .list_sessions(&params, (&self.workspace, &self.workspace_path))
            .await?;
        let mut sessions = records
            .iter()
            .map(|record| record.projection(params.workspace.as_ref()))
            .collect::<Vec<_>>();
        if params.session_ids.is_none() {
            let stored: HashSet<&str> = records.iter().map(|r| r.id.as_str()).collect();
            sessions.extend(self.live_beyond_limit(&params, &stored));
        }
        let result = json!({"sessions":sessions});
        ensure!(
            serde_json::to_vec(&result)?.len() <= MAX_LIST_BYTES,
            "Session list exceeds frame budget; use a smaller limit or sessionIds batch"
        );
        Ok(result)
    }
    /// TS listSessions 追加 `context.sessions` 中已持久化、未进入存储结果的活跃 runtime（例如超出 limit 的
    /// 较早会话）。映射同 TS `mapSessionInfo({app})`：无持久化行，因此标题为空、时间取当前时刻、
    /// mode/model/traceId 取运行时；不按归档过滤（TS 同样不查）。
    fn live_beyond_limit(&self, params: &ListParams, stored: &HashSet<&str>) -> Vec<Value> {
        if params
            .workspace
            .as_ref()
            .is_some_and(|w| w.workspace_key != self.workspace)
        {
            return vec![];
        }
        let now = self.clock.now();
        let mut live: Vec<_> = self
            .sessions
            .values()
            .filter(|s| {
                s.phase != "draft"
                    && LIST_TYPES.contains(&s.task_type.as_str())
                    && !stored.contains(s.id.as_str())
            })
            .collect();
        // TS Map 按 runtime 建立顺序迭代；Rust 常驻表按 id 排序，以创建时间近似建立顺序。
        live.sort_by_key(|s| (s.created_at, s.id.clone()));
        live.into_iter()
            .map(|s| {
                let mut workspace =
                    json!({"workspacePath":self.workspace_path,"workspaceKey":self.workspace});
                if self.workspace != self.workspace_path {
                    workspace["workspaceIdentity"] = self.workspace.clone().into();
                }
                let mut value = json!({"sessionId":s.id,"workspace":workspace,"sessionKind":s.task_type,
                    "title":"","mode":s.mode,"status":"idle","createdAt":now,"updatedAt":now});
                if !s.provider.is_empty() && !s.model.is_empty() {
                    value["model"] = json!({"providerId":s.provider,"modelId":s.model});
                }
                if let Some(trace) = &s.trace_id {
                    value["traceId"] = trace.clone().into();
                }
                if let Some(parent) = &s.parent_id {
                    value["parentSessionId"] = parent.clone().into();
                }
                value
            })
            .collect()
    }
}
