//! `escode-session.resumeSession` 的索引 model 列同步（TS `escodeTaskIndexSyncer.syncTaskModel`）。
//!
//! `broadcastSnapshot: false` + `model` 时 TS 不广播快照、只改索引 model 列：发送前 resume
//! 关闭快照广播是为了避免旧终态覆盖本地 streaming UI，但 sqlite 的 model 仍必须从已删除的
//! 历史模型更新到本次实际使用的可用模型（否则下次恢复继续从坏模型起跳）。

use serde_json::Value;

use super::domains_session_write::SessionWriteContext;
use crate::domains_task_support::{context_ref, workspace_identity_of, workspace_path_of};

/// TS `syncTaskModel`：trim 后非空才写；行缺失/索引不可用吞错只 warn。
pub(super) async fn sync_task_model(
    context: &SessionWriteContext,
    routing: &Value,
    params: &Value,
    model: Option<&str>,
) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };
    let Some(read) = context.read.as_ref() else {
        return;
    };
    let read = context_ref(read);
    let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
    let workspace_path = workspace_path_of(routing);
    let workspace_identity = workspace_identity_of(routing);
    let model = model.to_owned();
    let indexed = read
        .run(move |index| {
            index
                .update_task_state(
                    &escode_host_services::tasks_index::write_state::TaskRef {
                        workspace_path: &workspace_path,
                        workspace_identity: workspace_identity.as_deref(),
                        task_id: &session_id,
                    },
                    &escode_host_services::tasks_index::write_state::StatePatch {
                        model: Some(model),
                        ..Default::default()
                    },
                )
                .map_err(|error| error.to_string())
        })
        .await;
    if let Err((code, message)) = indexed {
        eprintln!("escode-host: task model sync failed: {code}: {message}");
    }
}
