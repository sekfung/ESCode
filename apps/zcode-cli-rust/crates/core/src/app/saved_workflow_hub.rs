//! 已保存工作流的 GUI 中枢（docs/specs/rust-dynamic-workflow.md 第 2 期），对齐 TS
//! `bootstrap/src/zcode-protocol/saved-workflows.ts`：workspace 级、无会话的方法。
//!
//! 与 `skills/referenceCatalog` 同一条先例：不带 sessionId，每次调用现扫目录——挂载时快照会漏掉
//! 用户手改或模型刚 SaveWorkflow 落盘的文件。解析/序列化只走存储层，这里不碰 frontmatter。
//! `workflows/runs` 是 run 历史（journal，第 4 期），不在这里。

use super::Engine;
use anyhow::Result;
use serde_json::Value;

impl Engine {
    pub(super) async fn saved_workflow_op(&mut self, op: &str, p: &Value) -> Result<Value> {
        self.validate_workspace(p)?;
        self.tools.saved_workflow_op(op, p).await
    }
}
