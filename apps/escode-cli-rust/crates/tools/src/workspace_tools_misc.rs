//! 部分 ToolPort 方法体（从 tools.rs 拆出）：入口体的转发目标，便于单文件保持在 400 行以内。

use super::tool_args::check_cancel;
use super::workspace_tools::WorkspaceTools;
use crate::contract::{EventSink, ToolOutput};
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_workflow(
    tools: &WorkspaceTools,
    session: &str,
    call_id: &str,
    name: &str,
    args: &Value,
    selection: &Value,
    mode: &str,
    model_catalog: Option<&[Value]>,
    input_adjustments: Option<&Value>,
    cancel: &CancellationToken,
) -> Option<Result<ToolOutput>> {
    if super::workflow_tools::ACTOR_TOOLS.contains(&name) {
        return Some(
            super::workflow_tools::execute_actor_tool(tools, session, call_id, name, args).await,
        );
    }
    if !super::workflow_host::routes(name, args) {
        return None;
    }
    Some(
        super::workflow_tools::execute(
            tools,
            session,
            call_id,
            name,
            args,
            selection,
            mode,
            model_catalog,
            input_adjustments,
            cancel,
        )
        .await,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn write_agent_artifacts(
    tools: &WorkspaceTools,
    parent_session: &str,
    task: &crate::domain::subagent::Task,
    profile: Option<&crate::domain::subagent::Profile>,
    cwd: &str,
    workspace_root: &str,
    text: &str,
    extra: &Value,
) -> Result<String> {
    // Node createSubagentLifecycle：<storageRoot>/cli/agents/<parent>/<agent>/。
    let dir = tools.subagent_root.join(parent_session).join(&task.id);
    tokio::fs::create_dir_all(&dir).await?;
    let output_file = dir.join("output.txt");
    let task_output_file = dir.join("task.output");
    let metadata_file = dir.join("metadata.json");
    write_atomic(&output_file, text).await?;
    write_atomic(&task_output_file, text).await?;

    // Node writeAgentMetadataFile / writeStoppedAgentArtifacts 的状态与字段集合。
    let status = match task.status.as_str() {
        "cancelled" | "stopped" => "stopped",
        other => other,
    };
    let updated_at = iso_now();
    let mut metadata = if status == "stopped" {
        serde_json::json!({
            "agentId": task.id,
            "childSessionId": task.child_id,
            "completedAt": updated_at,
            "description": task.description,
            "outputFile": output_file.to_string_lossy(),
            "parentSessionId": parent_session,
            "parentToolUseId": task.call_id,
            "profileId": task.agent_type,
            "prompt": task.prompt,
            "status": status,
            "taskOutputFile": task_output_file.to_string_lossy(),
            "updatedAt": updated_at,
        })
    } else {
        serde_json::json!({
            "agentId": task.id,
            "childSessionId": task.child_id,
            "createdAt": iso_millis(task.started_at),
            "cwd": cwd,
            "description": task.description,
            "metadataFile": metadata_file.to_string_lossy(),
            "outputFile": output_file.to_string_lossy(),
            "parentSessionId": parent_session,
            "parentToolUseId": task.call_id,
            "profileId": task.agent_type,
            "profileSnapshot": profile.and_then(|p| serde_json::to_value(p).ok()).unwrap_or(Value::Null),
            "prompt": task.prompt,
            "status": status,
            "taskOutputFile": task_output_file.to_string_lossy(),
            "updatedAt": updated_at,
            "workspaceRoot": workspace_root,
        })
    };
    if let Some(target) = metadata.as_object_mut() {
        let completed_at = task
            .ended_at
            .map(iso_millis)
            .unwrap_or_else(|| updated_at.clone());
        match status {
            "completed" => {
                target.insert("completedAt".into(), completed_at.into());
                target.insert(
                    "totalDurationMs".into(),
                    task.ended_at
                        .unwrap_or(task.started_at)
                        .saturating_sub(task.started_at)
                        .into(),
                );
                target.insert("totalTokens".into(), task.tokens.into());
                target.insert("totalToolUseCount".into(), task.tool_uses.into());
            }
            "failed" => {
                target.insert("completedAt".into(), completed_at.into());
                target.insert("error".into(), text.into());
            }
            _ => {}
        }
        if status != "stopped"
            && let Some(fields) = extra.as_object()
        {
            for (key, value) in fields {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    write_atomic(
        &metadata_file,
        &format!("{}\n", serde_json::to_string_pretty(&metadata)?),
    )
    .await?;
    Ok(output_file.to_string_lossy().into_owned())
}

async fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let temp = path.with_file_name(format!("{name}.tmp"));
    tokio::fs::write(&temp, text).await?;
    tokio::fs::rename(temp, path).await?;
    Ok(())
}

fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn iso_millis(millis: u64) -> String {
    chrono::DateTime::from_timestamp_millis(millis as i64)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(iso_now)
}

/// 权限配置：与 TS 同源（`~/.escode/cli/config.json` + 项目 escode.json/.escode/config.json，
/// 合并后取 `permission` 段）。CLI 的 --allowed-tools/--disallowed-tools 在 TS 侧也投影到这段。
pub(super) async fn permission_config(tools: &WorkspaceTools) -> crate::domain::permission::Config {
    let mut config = super::extension_config::permission_config(&tools.cwd).await;
    // 官方 serial / rea 插件的 defaultAllowedTools 免审批（TS resolveOfficialPluginDefaultAllowedTools）。
    config
        .allowed
        .extend(super::mcp_serial::default_allowed_tools(
            super::mcp_serial::host_serial_available(),
        ));
    config.allowed.extend(super::mcp_rea::default_allowed_tools(
        super::mcp_rea::host_rea_available(),
    ));
    config
}

pub(super) async fn prepare_tool(
    tools: &WorkspaceTools,
    session: &str,
    name: &str,
    args: &Value,
    skill_loaded: bool,
    model_catalog: Option<&[Value]>,
) -> Result<Option<std::result::Result<(Value, bool), String>>> {
    let (cwd, analyzer) = (&tools.workspace_path, &tools.analyzer);
    match name {
        super::save_workflow::TOOL => {
            super::save_workflow::prepare(cwd, args, skill_loaded, analyzer).await
        }
        super::eval_workflow_snippet::TOOL => {
            super::eval_workflow_snippet::prepare(cwd, args, skill_loaded, analyzer).await
        }
        name if super::workflow_host::TOOLS.contains(&name) => {
            return super::workflow_tools::prepare(
                tools,
                session,
                name,
                args,
                skill_loaded,
                model_catalog,
            )
            .await
            .map(Some);
        }
        _ => return Ok(None),
    }
    .map(Some)
}

pub(super) async fn execute_mcp(
    tools: &WorkspaceTools,
    name: &str,
    args: &Value,
    call_id: &str,
    meta: &Value,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    check_cancel(cancel)?;
    let artifacts = super::mcp_image_artifacts::ImageArtifacts {
        root: &tools.artifacts,
        session: &sink.session_id,
        call_id,
    };
    tools
        .mcp
        .call(&sink.session_id, name, args, meta, Some(artifacts), cancel)
        .await
}
