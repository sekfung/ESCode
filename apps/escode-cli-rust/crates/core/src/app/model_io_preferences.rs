//! `workspace/updateModelIoPreferences`（docs/specs/rust-model-io.md 第 2 期）：App 全局偏好「完整保留模型 IO」。
//! 与 TS 一样是进程级状态，对已有与之后的会话立即生效；回显 `{workspace, fullRetentionEnabled, updatedSessionCount}`。
use super::Engine;
use anyhow::{Context, Result};
use serde_json::{Value, json};

impl Engine {
    pub(super) fn model_io_preferences(&mut self, p: &Value) -> Result<Value> {
        self.validate_workspace(p)?;
        let enabled = p["preferences"]["fullRetentionEnabled"]
            .as_bool()
            .context("Invalid model IO preferences")?;
        crate::contract::set_model_io_full_retention(enabled);
        Ok(json!({
            "workspace": p["workspace"],
            "fullRetentionEnabled": enabled,
            // TS 统计常驻会话（每个持有独立 adapter）；Rust 的偏好是进程级，等价于当前内存中的会话数。
            "updatedSessionCount": self.sessions.len(),
        }))
    }
}
