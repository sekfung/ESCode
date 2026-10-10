//! 模型端口（`ModelPort` / `ModelRegistry`）与文件回退事务。

#[allow(unused_imports)]
use super::contract::*;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait ModelPort: Send + Sync {
    fn identity(&self) -> Option<ModelIdentity> {
        None
    }
    fn format_properties(&self) -> Value {
        serde_json::json!({"inputFormat":{"supportsText":true,"supportsImage":false,"supportsVideo":false,"supportsAudio":false,"supportsPdf":false},"outputFormat":{"supportsText":true}})
    }
    /// 模型是否声明 provider-native 搜索（TS `properties.supportsNativeWebSearch`）。
    fn native_web_search(&self) -> bool {
        false
    }
    fn with_max_output_tokens(&self, _max: usize) -> Result<Option<Arc<dyn ModelPort>>> {
        Ok(None)
    }
    fn bind(&self) -> Option<Arc<dyn ModelPort>> {
        None
    }
    /// 辅助调用用的最低推理档位绑定（TS `auxiliaryModelOptions`）；无注册表时为 None，沿用当前模型。
    fn auxiliary(&self) -> Option<Arc<dyn ModelPort>> {
        None
    }
    fn context_policy(&self) -> escode_cli_domain::context::ContextPolicy {
        Default::default()
    }
    async fn complete(
        &self,
        messages: Vec<Value>,
        tools: &[Value],
        sink: &EventSink,
        cancel: &CancellationToken,
    ) -> std::result::Result<ModelOutput, ModelFailure>;
}

/// Runtime configuration is resolved outside the actor; credentials are never session facts.
#[async_trait]
pub trait ModelRegistry: Send + Sync {
    async fn received_account(&self) -> Option<Value> {
        None
    }
    fn catalog(&self) -> Vec<Value>;
    /// `ListModels` 的目录面（TS `createModelCatalogPort`，docs/specs/rust-dynamic-workflow.md 第 6 期）：
    /// providerId/modelId/providerLabel?/reasoningLevels/defaultReasoningLevel?/contextWindow?。
    fn model_catalog(&self) -> Vec<Value> {
        Vec::new()
    }
    fn model_options(&self) -> Vec<Value> {
        self.catalog()
    }
    fn default_selection(&self) -> Option<ModelIdentity>;
    fn resolve(&self, selection: &ModelIdentity) -> Result<Arc<dyn ModelPort>>;
    async fn refresh(&self, account: Option<Value>) -> Result<bool>;
}

#[async_trait]
pub trait RewindTransaction: Send {
    fn preview(&self) -> Value;
    fn checkpoint_ids(&self) -> Vec<String>;
    async fn finish(self: Box<Self>, commit: bool) -> Result<()>;
}
