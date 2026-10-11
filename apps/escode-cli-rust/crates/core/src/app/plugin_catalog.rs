//! `plugins/referenceCatalog(WithCategory)`（docs/specs/rust-plugins.md 第 3 期）：对话 Picker 的插件引用目录。
//! 带 `sessionId` → 该会话冻结的身份目录（首次查询时冻结，进程内有效，会话关闭即丢弃；TS 在 App 创建时冻结、
//! 冷恢复重建）；不带 → workspace 当前目录。会话不存在时 fail closed，不回退 workspace 权威。
use super::Engine;
use anyhow::{Result, ensure};
use serde_json::{Value, json};

impl Engine {
    pub(super) async fn plugin_reference_catalog(
        &mut self,
        method: &str,
        p: &Value,
    ) -> Result<Value> {
        self.validate_workspace(p)?;
        let include_category = method.ends_with("WithCategory");
        let Some(id) = p["sessionId"].as_str() else {
            let result = self
                .tools
                .plugin_reference_catalog(p, None, include_category)
                .await?;
            return Ok(json!({ "authority": "workspace", "plugins": result["plugins"] }));
        };
        ensure!(!self.closed.contains(id), "Session unavailable");
        self.ensure_session(id).await?;
        let frozen = self.plugin_catalogs.get(id).cloned();
        let result = self
            .tools
            .plugin_reference_catalog(p, frozen.as_ref(), include_category)
            .await?;
        self.plugin_catalogs
            .entry(id.to_owned())
            .or_insert_with(|| result["identity"].clone());
        Ok(json!({ "authority": "session", "plugins": result["plugins"] }))
    }
}
