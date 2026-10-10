//! 已保存工作流的 GUI 中枢（docs/specs/rust-dynamic-workflow.md 第 2 期），对齐 TS
//! `bootstrap/src/escode-protocol/saved-workflows.ts`：workspace 级、无会话的方法。
//!
//! 与 `skills/referenceCatalog` 同一条先例：不带 sessionId，每次调用现扫目录——挂载时快照会漏掉
//! 用户手改或模型刚 SaveWorkflow 落盘的文件。解析/序列化只走存储层，这里不碰 frontmatter。
//! `workflows/runs` 是 run 历史（journal，第 4 期），不在这里。

use super::Engine;
use crate::domain::dwf_journal;
use anyhow::{Context, Result};
use serde_json::Value;

impl Engine {
    pub(super) async fn saved_workflow_op(&mut self, op: &str, p: &Value) -> Result<Value> {
        self.validate_workspace(p)?;
        self.tools.saved_workflow_op(op, p).await
    }

    /// `workflows/runs`（TS `listSavedWorkflowRunsOp`）：只读 journal，按 `dwf_run.name` 归属到工作流。
    /// journal 缺席（默认端口实现）回空页——TS 同款，中枢据此显示「尚未运行」。
    /// `project`（缺省）只查 `dwf_run.cwd === workspacePath`；`global` 不按 cwd 过滤，跨所有项目。
    pub(super) async fn saved_workflow_runs(&mut self, p: &Value) -> Result<Value> {
        self.validate_workspace(p)?;
        let limit = p["limit"].as_u64().filter(|value| (1..=50).contains(value)).context("Invalid limit")?;
        let global = p.get("scope").and_then(Value::as_str) == Some("global");
        let query = dwf_journal::RunQuery {
            cwd: if global {
                None
            } else {
                Some(
                    p["workspace"]["workspacePath"]
                        .as_str()
                        .context("Workspace path required")?
                        .to_owned(),
                )
            },
            name: p.get("name").and_then(Value::as_str).map(str::to_owned),
            statuses: None,
            // 多取一条**只为判定 truncated**（run service 与 v4 事件分页的同一惯例）。
            limit: limit as i64 + 1,
        };
        let rows = self.store.workflow_runs(&query).await?;
        let (runs, truncated) = dwf_journal::protocol_page(&rows, limit as usize);
        let mut value = crate::domain::json_order::Json::object();
        value.set("runs", crate::domain::json_order::Json::Array(runs));
        if truncated {
            value.set("truncated", crate::domain::json_order::Json::Bool(true));
        }
        Ok(dwf_journal::to_value(&value))
    }
}
