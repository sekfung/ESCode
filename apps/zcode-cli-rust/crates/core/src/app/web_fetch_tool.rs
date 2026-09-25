//! WebFetch 的会话侧处理：抓取交给 ToolPort，正文经辅助模型按 prompt 提炼（TS `processFetchedContent`）。
//! 见 docs/specs/rust-webfetch.md。

use crate::contract::{EventSink, ModelPort, ToolOutput, ToolPort};
use crate::domain::web_fetch as rules;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub(super) async fn execute(
    tools: &dyn ToolPort,
    model: &dyn ModelPort,
    args: &Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let started = Instant::now();
    let page = tools.web_fetch(args, cancel).await?;
    let mut output = page.output;
    let Some(content) = page.content else {
        return Ok(finish(output));
    };
    let content_type = output["contentType"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let (result, truncated) =
        if rules::returns_markdown_directly(page.preapproved, &content_type, &content) {
            (content, false)
        } else {
            let (body, truncated) = rules::truncate_for_model(&content);
            let prompt = args["prompt"].as_str().unwrap_or_default();
            let messages = vec![json!({
                "role": "user",
                "content": rules::processing_prompt(&body, prompt, page.preapproved),
            })];
            // TS auxiliaryModelOptions：最低推理档位，maxOutputTokens = min(4096, 模型上限)。
            let auxiliary = model.auxiliary().or_else(|| model.bind());
            let base = auxiliary.as_deref().unwrap_or(model);
            let limited = base.with_max_output_tokens(rules::MAX_PROCESSING_OUTPUT_TOKENS)?;
            let processing = limited.as_deref().unwrap_or(base);
            let reply = super::context::hidden_summary(processing, messages, sink, cancel)
                .await
                .map_err(|error| anyhow!("{error}"))?;
            let text = reply.message["content"].as_str().unwrap_or_default();
            (rules::processing_result(text), truncated)
        };
    output["result"] = result.into();
    output["truncated"] = truncated.into();
    output["durationMs"] = json!(started.elapsed().as_millis() as u64);
    Ok(finish(output))
}

/// 模型可见内容为 result（TS formatWebFetchModelContent）。
fn finish(output: Value) -> ToolOutput {
    let content = output["result"].as_str().unwrap_or_default().to_owned();
    ToolOutput::new(content, output)
}
