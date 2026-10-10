//! `ListWorkflowRuns`（docs/specs/rust-dynamic-workflow.md 第 6 期）：按项目（会话工作目录）枚举
//! workflow run，含跨会话历史。工具在会话回合里执行，journal 由 owner 的存储持有——经
//! `Event::WorkflowRunList` 读回，投影在 `domain::workflow_run_list`（纯逻辑）。
//!
//! cwd 恒取本会话的工作目录：模型无权跨项目扫库（TS handler 同），这同时是 `sideEffectScope: "none"`
//! 成立的前提。
use crate::contract::{Event, EventSink, ToolOutput};
use crate::domain::dwf_journal::{self, RunQuery};
use crate::domain::workflow_run_list;
use anyhow::{Context, Result, bail};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// TS 输入 schema：limit 缺省 20 并钳到 [1, 50]（界已在 schema 面钳好，这里只取缺省）。
const DEFAULT_LIMIT: u64 = 20;
/// TS `WORKFLOW_RUN_INTROSPECTION_ERROR_CODE.INTROSPECTION_UNAVAILABLE`：能力缺席的业务失败。
const INTROSPECTION_UNAVAILABLE: &str = "workflow_introspection_unavailable: this session cannot read workflow runs — workflow execution is not available here, so no run history is reachable. This is a capability gap, not an empty project.";

pub(super) async fn execute(
    args: &serde_json::Value,
    cwd: Option<&str>,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let Some(cwd) = cwd else {
        return Ok(unavailable());
    };
    let limit = args["limit"].as_u64().filter(|value| (1..=50).contains(value)).unwrap_or(DEFAULT_LIMIT);
    let statuses = args["statuses"]
        .as_array()
        .map(|values| values.iter().filter_map(serde_json::Value::as_str).map(str::to_owned).collect());
    let query = RunQuery {
        cwd: Some(cwd.to_owned()),
        name: None,
        statuses,
        // 多取一条**只为判定 truncated**（端口同款）。
        limit: limit as i64 + 1,
    };
    let (reply, rows) = oneshot::channel();
    sink.send(Event::WorkflowRunList { query, reply }).await?;
    let rows = tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        rows = rows => rows.context("Session owner stopped before reading the workflow journal")?,
    };
    let rows = match rows {
        Ok(rows) => rows,
        // 存储读失败（journal 不可用）：对模型是"本会话没有这个能力"，不是空项目。
        Err(_) => return Ok(unavailable()),
    };
    let truncated = rows.len() as u64 > limit;
    let page = if truncated { &rows[..limit as usize] } else { &rows[..] };
    let items = page
        .iter()
        .map(|entry| workflow_run_list::item(&entry.row, &sink.session_id))
        .collect::<Vec<_>>();
    let output = workflow_run_list::output(&items, truncated);
    let mut result = ToolOutput::new(
        workflow_run_list::model_content(&output),
        dwf_journal::to_value(&output),
    );
    result.display = Some(dwf_journal::to_value(&workflow_run_list::display(&output)));
    Ok(result)
}

/// TS `workflowIntrospectionUnavailableFailure`：能力缺口，不静默回空列表。
fn unavailable() -> ToolOutput {
    ToolOutput {
        failed: true,
        ..ToolOutput::text(INTROSPECTION_UNAVAILABLE.to_owned())
    }
}
