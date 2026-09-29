//! `session/resume`：App 的会话/任务恢复入口（TS `server-operations.ts::resumeSession` +
//! `activateSessionForResume`）。见 docs/specs/rust-session-loading.md。

use super::Engine;
use anyhow::{Context, Result};
use serde_json::Value;

impl Engine {
    /// 冷恢复：物化 runtime（`ensure_session`）→ 应用恢复参数 → 返回与 `session/read` 同形的 snapshot。
    ///
    /// 与 TS 一致，**会话已经活跃时不重放参数**（`activateSessionForResume` 命中 existing 直接返回）。
    pub(super) async fn resume_session(&mut self, p: &Value) -> Result<Value> {
        self.validate_workspace(p)?;
        let id = p["sessionId"].as_str().context("Session id required")?.to_owned();
        let was_active = self.sessions.contains_key(&id);
        // TS：持久化记录不存在时报 "Session not found"（与 requireSession 的「未活跃」区分）。
        self.ensure_session(&id).await?;
        if !was_active {
            // 冷恢复重建 runtime 时必须沿用 create 的工具面约束，否则恢复出来的会话会丢
            // OffPeak / 工作流工具簇（TS resume 与 create 同语义）。
            if let Some(enabled) = p["offPeakToolEnabled"].as_bool() {
                self.fix_off_peak(&id, enabled);
            }
            if let Some(enabled) = p["dynamicWorkflowEnabled"].as_bool() {
                self.fix_dynamic_workflow(&id, enabled);
            }
            if let Some(servers) = p.get("mcpServers") {
                self.tools.configure_mcp(&id, servers).await?;
            }
            // 旧会话没有持久化推理档位时，用同 task 索引里的 hint 迁移（TS `thoughtLevel` hint）；
            // 已有档位以持久化事实为准，不被请求覆盖。
            let hint = p["thoughtLevel"]
                .as_str()
                .map(str::trim)
                .filter(|level| !level.is_empty())
                .map(str::to_owned);
            if let Some(level) = hint
                && let Some(session) = self.sessions.get_mut(&id)
                && session.reasoning_level.is_empty()
            {
                session.reasoning_level = level;
                session.revision += 1;
            }
        }
        self.read_session(p)
    }
}
