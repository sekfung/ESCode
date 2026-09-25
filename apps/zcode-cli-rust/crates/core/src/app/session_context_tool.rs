//! ReadSessionContext：会话读取交给 owner（存储），素材与辅助模型抽取由 domain::session_context 完成。
//! 见 docs/specs/rust-read-session-context.md。

use crate::contract::{Event, EventSink, ModelPort, ToolOutput};
use crate::domain::session_context::{
    Input, LiteCall, failed_output, model_content, not_found_output, run,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) async fn execute(
    model: &dyn ModelPort,
    args: &Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let input = Input::parse(args).map_err(|message| anyhow!(message))?;
    let (reply, loaded) = oneshot::channel();
    sink.send(Event::SessionContext {
        id: input.session_id.clone(),
        reply,
    })
    .await?;
    let loaded = tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        loaded = loaded => loaded.context("Session owner stopped before reading session history")?,
    };
    let output = match loaded {
        Err(error) => failed_output(&input, &format!("{error:#}")),
        Ok(None) => not_found_output(&input),
        Ok(Some((info, messages))) => {
            // TS auxiliaryModelOptions：最低推理档；每次调用的 maxOutputTokens 由 with_max_output_tokens 按模型上限收紧。
            let auxiliary = model.auxiliary().or_else(|| model.bind());
            let base = auxiliary.as_deref().unwrap_or(model);
            run(
                &input,
                &info,
                &messages,
                Some(usize::MAX),
                |call: LiteCall| {
                    let limited = base.with_max_output_tokens(call.max_output_tokens);
                    async move {
                        let limited = limited.map_err(|error| error.to_string())?;
                        let processing = limited.as_deref().unwrap_or(base);
                        let reply =
                            super::context::hidden_summary(processing, call.messages, sink, cancel)
                                .await
                                .map_err(|error| error.to_string())?;
                        Ok(reply.message["content"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned())
                    }
                },
            )
            .await
        }
    };
    Ok(ToolOutput::new(model_content(&output), output))
}
