//! `WikiPort` 实现：core 经此访问 Wiki；适配器内部错误按端口契约返回给调用方处理（自动检索由 core fail-open）。
use super::service::WikiService;
use anyhow::Result;
use async_trait::async_trait;
use escode_cli_core_api::{
    WikiPort, WikiSurface, WikiToolResult, WikiTurnInput, WikiTurnRetrieval,
};
use escode_cli_domain::wiki::config::AgentTools;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[async_trait]
impl WikiPort for WikiService {
    async fn surface(&self) -> WikiSurface {
        let settings = self.settings().await;
        if !settings.config.enabled {
            return WikiSurface::default();
        }
        let has_knowledge = match self.open_snapshots().await {
            Ok(snapshots) => snapshots.iter().any(|s| !s.entries.is_empty()),
            Err(_) => false,
        };
        let tools = match settings.config.agent_tools {
            AgentTools::Always => true,
            AgentTools::Never => false,
            AgentTools::Auto => has_knowledge,
        };
        WikiSurface {
            enabled: true,
            tools,
            auto_inject: settings.config.auto_inject && has_knowledge,
        }
    }

    async fn retrieve_for_turn(
        &self,
        input: WikiTurnInput,
        cancel: &CancellationToken,
    ) -> Result<WikiTurnRetrieval> {
        let timeout =
            std::time::Duration::from_millis(self.settings().await.config.retrieval.timeout_ms);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => anyhow::bail!("Cancelled"),
            result = tokio::time::timeout(timeout, self.retrieve(&input.session_id, &input.prompt, &input.already_injected)) => {
                result.map_err(|_| anyhow::anyhow!("wiki retrieval exceeded wiki.retrieval.timeoutMs ({} ms)", timeout.as_millis()))?
            }
        }
    }

    async fn tool(&self, name: &str, input: &Value, session_id: &str) -> Result<WikiToolResult> {
        WikiService::tool(self, name, input, session_id).await
    }

    async fn request(&self, method: &str, params: &Value) -> Result<Value> {
        WikiService::request(self, method, params).await
    }
}
