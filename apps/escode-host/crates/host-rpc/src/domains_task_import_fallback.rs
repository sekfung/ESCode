//! Claude 导入的 legacy 落盘分支：Rust runtime 只接受 `source="sharedContext"` 的
//! importedHistory，不承接 Claude 消息数组；导入退回 TS `persistImportedClaudeTask` 的
//! 快照 + 索引写面（会话可打开为只读历史，续聊需运行时后续支持）。

use std::path::Path;

use escode_host_services::claude_import::{self, ImportedSession};
use escode_host_services::tasks_index::SyncOptions;
use serde_json::{Value, json};

use crate::domains_task_state::StateContext;

pub(super) async fn persist_legacy(
    context: &StateContext,
    data: &Path,
    source: &ImportedSession,
    target_identity: Option<&str>,
) -> Result<Value, String> {
    let task_id = claude_import::build_task_id(&source.workspace_path, &source.session_id);
    let title = if source.title.trim().is_empty() {
        "Imported session".to_owned()
    } else {
        source.title.clone()
    };
    let mut meta = json!({
        "taskId": task_id,
        "traceId": uuid::Uuid::new_v4().to_string(),
        "title": title,
        "workspacePath": source.workspace_path,
        "createdAt": source.created_at,
        "updatedAt": source.updated_at.max(source.created_at),
        "mode": "default",
        "provider": "glm",
        "migrationSource": "claudeCode",
        "status": "completed",
    });
    if let Some(identity) = target_identity {
        meta["workspaceIdentity"] = Value::from(identity);
    }
    let searchable = searchable_text(&source.messages);
    let sync_meta = meta.clone();
    let identity_owned = target_identity.map(str::to_owned);
    let meta = context
        .read
        .run(move |index| {
            let mut sync_meta = sync_meta;
            if let Some(identity) = identity_owned.as_ref() {
                sync_meta["workspaceIdentity"] = Value::from(identity.clone());
            }
            let Some(record) = sync_meta.as_object() else {
                return Err("imported meta is not an object".to_owned());
            };
            index
                .sync_task_meta(
                    record,
                    &SyncOptions {
                        archived: Some(false),
                        deleted: Some(false),
                        searchable_text: Some(searchable),
                        ..SyncOptions::default()
                    },
                )
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|(_, error)| error)?;
    context.read.targets.remember_indexed_meta(
        &task_id,
        &source.workspace_path,
        target_identity,
        None,
    );

    let data = data.to_path_buf();
    let workspace = source.workspace_path.clone();
    let identity = target_identity.map(str::to_owned);
    let task = task_id.clone();
    let meta_for_file = meta.clone();
    let messages = source.messages.clone();
    crate::domains_file::spawn_blocking_io(move || {
        claude_import::write_legacy_snapshot(
            &data,
            &workspace,
            identity.as_deref(),
            &task,
            &meta_for_file,
            &messages,
        )
        .map(|_| ())
        .map_err(|error| ("Error".to_owned(), error))
    })
    .await
    .map_err(|(_, error)| error)?;
    Ok(meta)
}

fn searchable_text(messages: &[Value]) -> String {
    let mut text = String::new();
    for message in messages {
        let content = message["content"].as_str().unwrap_or_default().trim();
        if content.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(content);
        if text.len() >= 200_000 {
            break;
        }
    }
    text.chars().take(200_000).collect()
}
