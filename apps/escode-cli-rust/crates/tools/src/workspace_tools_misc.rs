//! 部分 ToolPort 方法体（从 tools.rs 拆出）：入口体的转发目标，便于单文件保持在 400 行以内。

use super::tool_args::check_cancel;
use super::workspace_tools::WorkspaceTools;
use crate::contract::{EventSink, ToolOutput};
use anyhow::Result;
use serde_json::Value;
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
            tools, session, call_id, name, args, selection, mode, cancel,
        )
        .await,
    )
}

pub(super) async fn agent_output(
    tools: &WorkspaceTools,
    session: &str,
    text: &str,
) -> Result<String> {
    let dir = tools.artifacts.join(format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(session.as_bytes())
    ));
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("output.txt"); // TS createSubagentLifecycle 的文件名
    let temp = dir.join("output.txt.tmp");
    tokio::fs::write(&temp, text).await?;
    tokio::fs::rename(temp, &path).await?;
    Ok(path.to_string_lossy().into_owned())
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
            return super::workflow_tools::prepare(tools, session, name, args, skill_loaded)
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
