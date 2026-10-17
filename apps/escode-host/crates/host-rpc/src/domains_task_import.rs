//! `escode-task` Claude 历史导入（S10）：扫描 `~/.claude/projects`，复制 jsonl，创建真实
//! ESCode session 并写索引/legacy 快照（TS `claudeNativeSessionImport*` 的 Host 侧编排）。

use std::path::{Path, PathBuf};

use escode_host_services::claude_import;
use escode_host_services::session_params::{ToolGates, session_create_params};
use escode_host_services::task_projection::{
    IndexOverrides, index_meta_from_snapshot, searchable_text_from_snapshot,
};
use escode_host_services::tasks_index::SyncOptions;
use serde_json::{Value, json};

use crate::domains_task_input::runtime;
use crate::domains_task_state::{StateContext, method};
use crate::domains_task_support::{ReadContext, first_object};
use crate::forward::{self, Extract, Shaped};
use crate::session::{MethodHandler, handler};
use crate::wire::Wire;
use crate::workspace_events::Target;

pub(super) fn import_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        ("scanImportableClaudeSessions", scan(context)),
        ("importClaudeSessions", import(context)),
    ]
}

fn data_base_dir_from(read: &ReadContext) -> PathBuf {
    read.path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn scan(context: &StateContext) -> MethodHandler {
    let read = crate::domains_task_support::context_ref(&context.read);
    let home = context.home.clone();
    handler(move |arg: Wire| {
        let params = first_object(arg);
        let read = read.clone();
        let home = home.clone();
        Box::pin(async move {
            let data = data_base_dir_from(&read);
            let home = home.clone();
            let workspace = params["workspacePath"].as_str().map(str::to_owned);
            let modified_since = params["modifiedSince"].as_i64();
            let limit = params["limit"].as_u64().map(|value| value as usize);
            let candidates = crate::domains_file::spawn_blocking_io(move || {
                Ok::<_, (String, String)>(claude_import::scan_importable_sessions(
                    &home,
                    Some(&data),
                    workspace.as_deref(),
                    modified_since,
                    limit,
                ))
            })
            .await
            .map_err(|error| error.1)
            .map_err(|message| ("Fatal".to_owned(), message))?;
            Ok(Wire::Array(
                candidates
                    .iter()
                    .map(|candidate| Wire::Object(candidate.to_json()))
                    .collect(),
            ))
        })
    })
}

fn import(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let runtime = runtime(&context)?;
        let data = data_base_dir_from(&context.read);
        let filter_workspace = params["workspacePath"].as_str().map(str::to_owned);
        let target_identity = filter_workspace
            .as_ref()
            .and_then(|_| params["workspaceIdentity"].as_str().map(str::to_owned));
        let ids: Vec<String> = params["sessionIds"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        let mut imported = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();
        for session_id in ids {
            if !seen.insert(session_id.clone()) {
                continue;
            }
            match import_one(
                &context,
                &runtime,
                &data,
                &context.home,
                filter_workspace.as_deref(),
                target_identity.as_deref(),
                &session_id,
            )
            .await
            {
                Ok(meta) => {
                    let workspace_path = meta["workspacePath"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let workspace_identity = meta["workspaceIdentity"].as_str();
                    let task_id = meta["taskId"].as_str().unwrap_or_default();
                    context.events.task_list_changed(
                        &Target {
                            workspace_path: &workspace_path,
                            workspace_identity,
                            task_id: Some(task_id),
                        },
                        Some(&meta),
                        "task_meta_changed",
                        None,
                    );
                    imported.push(json!({
                        "provider": "claude",
                        "sessionId": session_id,
                        "taskId": meta["taskId"],
                        "workspacePath": meta["workspacePath"],
                    }));
                }
                Err(ImportFailure::Skipped { reason, workspace }) => {
                    let mut entry = json!({
                        "provider": "claude",
                        "sessionId": session_id,
                        "reason": reason,
                    });
                    if let Some(workspace) = workspace {
                        entry["workspacePath"] = Value::from(workspace);
                    } else if let Some(filter) = filter_workspace.as_ref() {
                        entry["workspacePath"] = Value::from(filter.clone());
                    }
                    skipped.push(entry);
                }
                Err(ImportFailure::Failed { reason }) => {
                    failed.push(json!({
                        "provider": "claude",
                        "sessionId": session_id,
                        "reason": reason,
                    }));
                }
            }
        }
        Ok(Wire::Object(json!({
            "imported": imported,
            "skipped": skipped,
            "failed": failed,
        })))
    })
}

enum ImportFailure {
    Skipped {
        reason: &'static str,
        workspace: Option<String>,
    },
    Failed {
        reason: String,
    },
}

impl From<String> for ImportFailure {
    fn from(reason: String) -> Self {
        Self::Failed { reason }
    }
}

async fn import_one(
    context: &StateContext,
    runtime: &crate::domains_task_support::RuntimeAccess,
    data: &Path,
    home: &Path,
    filter_workspace: Option<&str>,
    target_identity: Option<&str>,
    session_id: &str,
) -> Result<Value, ImportFailure> {
    let data_owned = data.to_path_buf();
    let home_owned = home.to_path_buf();
    let filter_owned = filter_workspace.map(str::to_owned);
    let known_session = session_id.to_owned();
    let candidate = crate::domains_file::spawn_blocking_io(move || {
        Ok::<_, (String, String)>(claude_import::find_importable_session(
            &home_owned,
            Some(&data_owned),
            filter_owned.as_deref(),
            &known_session,
        ))
    })
    .await
    .map_err(|error| ImportFailure::Failed { reason: error.1 })?
    .ok_or_else(|| ImportFailure::Skipped {
        reason: "session_not_found_or_workspace_mismatch",
        workspace: filter_workspace.map(str::to_owned),
    })?;
    if !Path::new(&candidate.workspace_path).exists() {
        return Err(ImportFailure::Skipped {
            reason: "workspace_path_missing",
            workspace: Some(candidate.workspace_path),
        });
    }

    let source_path = PathBuf::from(&candidate.source_path);
    let data_owned = data.to_path_buf();
    let workspace_for_copy = candidate.workspace_path.clone();
    let identity_for_copy = target_identity.map(str::to_owned);
    let copy = crate::domains_file::spawn_blocking_io(move || {
        claude_import::copy_session_file_to_workspace(
            &data_owned,
            &workspace_for_copy,
            identity_for_copy.as_deref(),
            &source_path,
        )
        .map_err(|error| ("Error".to_owned(), error))
    })
    .await
    .map_err(|error| ImportFailure::Failed { reason: error.1 })?;

    let parse_path = candidate.source_path.clone();
    let workspace = candidate.workspace_path.clone();
    let session = session_id.to_owned();
    let created_at = candidate.created_at;
    let updated_at = Some(candidate.updated_at);
    let source = match crate::domains_file::spawn_blocking_io(move || {
        claude_import::parse_session(
            Path::new(&parse_path),
            &workspace,
            &session,
            created_at,
            updated_at,
        )
        .map_err(|error| ("Error".to_owned(), error))
    })
    .await
    {
        Ok(source) => source,
        Err((_, reason)) => {
            cleanup(&copy.created_output_paths);
            return Err(ImportFailure::from(reason));
        }
    };

    let task_id = claude_import::build_task_id(&source.workspace_path, session_id);
    let trace_id = uuid::Uuid::new_v4().to_string();
    let mut create = json!({
        "sessionId": task_id,
        "sessionTraceId": trace_id,
        "persistence": "immediate",
        "importedHistory": {
            "source": "claudeCode",
            "title": source.title,
            "createdAt": source.created_at,
            "updatedAt": source.updated_at,
            "messages": source.messages.iter().map(|message| json!({
                "role": message["role"],
                "content": message["content"],
                "timestamp": message["timestamp"],
            })).collect::<Vec<_>>(),
        },
    });
    create["workspacePath"] = Value::from(source.workspace_path.clone());
    if let Some(identity) = target_identity {
        create["workspaceIdentity"] = Value::from(identity);
    }
    let routing = json!({
        "workspacePath": source.workspace_path,
        "workspaceIdentity": target_identity,
    });
    let shaped = Shaped {
        method: "session/create",
        body: session_create_params(&create, &ToolGates::default()),
        with_workspace: true,
        trace: Some(json!({"traceId": trace_id})),
    };
    let snapshot = match forward::runtime_call_shaped(
        &runtime.instances,
        &runtime.options,
        routing,
        shaped,
        Extract::Result,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) if is_unsupported_import(&error.message) => {
            return crate::domains_task_import_fallback::persist_legacy(
                context,
                data,
                &source,
                target_identity,
            )
            .await
            .map_err(|reason| ImportFailure::Failed { reason });
        }
        Err(error) => {
            cleanup(&copy.created_output_paths);
            return Err(ImportFailure::Failed {
                reason: error.message,
            });
        }
    };

    let Wire::Object(snapshot) = snapshot else {
        cleanup(&copy.created_output_paths);
        return Err(ImportFailure::Failed {
            reason: "session/create returned a non-object result".to_owned(),
        });
    };
    let snapshot_owned = snapshot.clone();
    let identity_owned = target_identity.map(str::to_owned);
    let meta = match context
        .read
        .run(move |index| {
            let mut meta = index_meta_from_snapshot(&snapshot_owned, &IndexOverrides::default());
            meta["migrationSource"] = Value::from("claudeCode");
            if let Some(identity) = identity_owned.as_ref() {
                meta["workspaceIdentity"] = Value::from(identity.clone());
            }
            let Some(record) = meta.as_object() else {
                return Err("imported meta is not an object".to_owned());
            };
            index
                .sync_task_meta(
                    record,
                    &SyncOptions {
                        archived: Some(false),
                        deleted: Some(false),
                        searchable_text: Some(searchable_text_from_snapshot(&snapshot_owned)),
                        ..SyncOptions::default()
                    },
                )
                .map_err(|error| error.to_string())
        })
        .await
    {
        Ok(meta) => meta,
        Err((_, reason)) => {
            cleanup(&copy.created_output_paths);
            return Err(ImportFailure::Failed { reason });
        }
    };
    context.read.targets.remember_indexed_meta(
        &task_id,
        &source.workspace_path,
        target_identity,
        None,
    );

    let data_owned = data.to_path_buf();
    let workspace_owned = source.workspace_path.clone();
    let identity_owned = target_identity.map(str::to_owned);
    let task_owned = task_id.clone();
    let meta_owned = meta.clone();
    let messages = source.messages.clone();
    let written = crate::domains_file::spawn_blocking_io(move || {
        claude_import::write_legacy_snapshot(
            &data_owned,
            &workspace_owned,
            identity_owned.as_deref(),
            &task_owned,
            &meta_owned,
            &messages,
        )
        .map(|_| ())
        .map_err(|error| ("Error".to_owned(), error))
    })
    .await;
    if let Err((_, reason)) = written {
        cleanup(&copy.created_output_paths);
        return Err(ImportFailure::Failed { reason });
    }
    Ok(meta)
}

fn is_unsupported_import(message: &str) -> bool {
    message.contains("Unsupported imported history source")
        || message.contains("Null shared import field")
}

fn cleanup(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}
