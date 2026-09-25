//! WebSearch 的会话侧执行（docs/specs/rust-websearch.md，对齐 TS `webSearchHandler`）：
//! 辅助模型带 provider-native 搜索工具发起一次隐藏请求，模型可见结果由流式文本整理。
use crate::contract::{EventSink, ModelPort, ToolOutput};
use crate::domain::web_search as rules;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// TS WebSearch `timeoutMs`。
const TIMEOUT: Duration = Duration::from_secs(60);
/// TS `Math.min(4096, 模型上限)`。
const MAX_OUTPUT_TOKENS: usize = 4096;

pub(super) async fn execute(
    model: &dyn ModelPort,
    args: &Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let started = Instant::now();
    let input = rules::validate(args).map_err(|error| anyhow!(error))?;
    if !model.native_web_search() {
        bail!("Current model does not support native WebSearch");
    }
    // TS auxiliaryModelOptions：最低推理档位。
    let auxiliary = model.auxiliary().or_else(|| model.bind());
    let base = auxiliary.as_deref().unwrap_or(model);
    let limited = base.with_max_output_tokens(MAX_OUTPUT_TOKENS)?;
    let search = limited.as_deref().unwrap_or(base);
    let (messages, tool) = rules::request(&input);
    let reply = tokio::time::timeout(
        TIMEOUT,
        super::context::hidden_complete(search, messages, &[tool], sink, cancel),
    )
    .await
    .map_err(|_| anyhow!("WebSearch timed out"))?
    .map_err(|error| anyhow!("{error}"))?;
    let text = reply.message["content"].as_str().unwrap_or_default();
    let mut output = rules::output(&input.query, text);
    output["durationMs"] = json!(started.elapsed().as_millis() as u64);
    Ok(ToolOutput::new(rules::model_content(&output), output))
}
