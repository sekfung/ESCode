//! 少数体量较大的 ToolPort 方法体；trait impl 仍集中在 `tools.rs`。

use anyhow::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::workspace_tools::WorkspaceTools;

pub(crate) async fn web_fetch(
    tools: &WorkspaceTools,
    args: &Value,
    session: &str,
    call_id: &str,
    cancel: &CancellationToken,
) -> Result<crate::contract::WebFetchPage> {
            super::web_fetch::fetch(
                &super::web_fetch::HttpTransport,
                args,
                Some(super::web_fetch::ArtifactTarget {
                    root: &tools.artifacts,
                    session,
                    call_id,
                }),
                cancel,
            )
            .await

}

pub(crate) async fn begin_rewind(
    tools: &WorkspaceTools,
    session: &str,
    token: &str,
    changes: &[escode_cli_domain::file_checkpoint::FileCheckpoint],
) -> Result<Box<dyn crate::contract::RewindTransaction>> {
    super::file_rewind::begin(
        &tools.artifacts,
        session,
        token,
        changes,
        tools.writes.clone(),
    )
    .await
}
