use crate::{
    contract::{Event, EventSink, ToolOutput},
    domain::question::QuestionInput,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub(super) async fn execute(
    call: &str,
    args: Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    // TS 交互 broker 对 refine 失败的入参直接拒绝，不等待用户：模型看到首条 issue，行按拒绝收口
    // （docs/specs/rust-user-questions.md「入参 refine 失败」；之前 Rust 以工具错误结束）。
    // 首条问题的顺序同 zod：各题 refine → annotations 值 → 输入层 refine。
    let annotation = crate::domain::question::annotation_issue(&args);
    let mut args = args;
    if annotation.is_some()
        && let Some(record) = args.as_object_mut()
    {
        record.remove("annotations");
    }
    let mut input = QuestionInput::structural(args)?;
    let issue = match (input.refine_questions(), annotation) {
        (Err(error), _) => Some(error.to_string()),
        (Ok(()), Some(issue)) => Some(issue),
        (Ok(()), None) => input.refine_input().err().map(|e| e.to_string()),
    };
    if let Some(error) = issue {
        return Ok(ToolOutput {
            media: Vec::new(),
            content: format!("Invalid AskUserQuestion input: {error}"),
            data: Value::Null,
            failed: true,
            display: None,
            control: crate::contract::ToolControl { denied: true, stop_turn: false },
        });
    }
    // 模型生成的 answers 不能替代用户回执；yolo 也必须进入 owner 的澄清等待。
    input.answers = None;
    input.annotations = None;
    let (reply, receipt) = tokio::sync::oneshot::channel();
    sink.send(Event::Question {
        call_id: call.into(),
        input: Box::new(input),
        reply,
    })
    .await?;
    let answer = tokio::select! {biased;
        _=cancel.cancelled()=>bail!("AskUserQuestion was cancelled before answers were returned"),
        answer=receipt=>answer.context("Question owner stopped before answer commit")?,
    };
    Ok(ToolOutput {
        media: Vec::new(),
        content: answer.content,
        data: answer.data,
        failed: answer.failed,
        display: None,
        control: Default::default(),
    })
}
