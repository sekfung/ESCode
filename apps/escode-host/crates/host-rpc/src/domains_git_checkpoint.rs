//! `git-checkpoint` channel（TS `ServiceChannels.GitCheckpoint` = `"git-checkpoint"`；docs/specs/rust-u2-git.md §8）。
//!
//! 四个成员：`createCheckpoint` / `diffCheckpoints` / `restoreBetweenCheckpoints` / `deleteCheckpoint`。
//! 业务实现全在 `escode_host_services::git::checkpoint_service`；这里只做参数解包与形状整形。
//! 与 `git` 通道共用同一 `GitService`（单所有者），manifest 存储由服务层持有。

use std::sync::Arc;

use escode_host_services::git::GitService;
use escode_host_services::git::checkpoint::FileDiff;
use escode_host_services::git::checkpoint_conflicts::Conflict;
use escode_host_services::git::checkpoint_service::GitCheckpointService;
use serde_json::{Value, json};

use crate::domains_task_support::first_object;
use crate::session::{Channel, handler};
use crate::wire::Wire;

/// TS 抛的是普通 `Error`：只带 `name`/`message`。
fn failed(error: anyhow::Error) -> (String, String) {
    ("Error".to_owned(), format!("{error:#}"))
}

fn file_json(file: &FileDiff) -> Value {
    // `originalPath` **恒在**（TS 在无 rename 时显式写 `null`，不是省略键）。
    json!({
        "path": file.path,
        "repoRelativePath": file.repo_relative_path,
        "workspaceRelativePath": file.workspace_relative_path,
        "originalPath": file.original_path,
        "kind": file.kind,
        "added": file.added,
        "removed": file.removed,
    })
}

fn conflict_json(conflict: &Conflict) -> Value {
    json!({
        "path": conflict.path,
        "repoRelativePath": conflict.repo_relative_path,
        "workspaceRelativePath": conflict.workspace_relative_path,
        "reason": conflict.reason,
    })
}

/// 缺 `dataBaseDir` 时**方法表照常登记**（契约对账要求「Implemented 必须能在表里找到」），
/// 调用一律回结构化缺口——manifest 与临时 index 根都定位不到，不做半迁移。
fn gap_channel() -> Channel {
    let gap = "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.";
    let missing = || {
        handler(move |_arg: Wire| {
            let gap = gap.to_owned();
            Box::pin(async move { Err(("InvalidRequest".to_owned(), gap)) })
        })
    };
    Channel::new(
        "git-checkpoint",
        vec![
            ("createCheckpoint", missing()),
            ("diffCheckpoints", missing()),
            ("restoreBetweenCheckpoints", missing()),
            ("deleteCheckpoint", missing()),
        ],
    )
}

pub(crate) fn git_checkpoint_channel(
    service: Arc<GitService>,
    data_base_dir: Option<std::path::PathBuf>,
) -> Channel {
    let Some(data_base_dir) = data_base_dir else {
        return gap_channel();
    };
    let checkpoints = Arc::new(GitCheckpointService::new(service, &data_base_dir));

    let create = {
        let checkpoints = Arc::clone(&checkpoints);
        handler(move |arg: Wire| {
            let params = first_object(arg);
            let checkpoints = Arc::clone(&checkpoints);
            Box::pin(async move {
                let workspace = params["workspacePath"].as_str().unwrap_or_default();
                let meta = checkpoints.create(workspace).await.map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Object(
                    serde_json::to_value(meta)
                        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))?,
                ))
            })
        })
    };

    let diff = {
        let checkpoints = Arc::clone(&checkpoints);
        handler(move |arg: Wire| {
            let params = first_object(arg);
            let checkpoints = Arc::clone(&checkpoints);
            Box::pin(async move {
                let files = checkpoints
                    .diff(
                        params["workspacePath"].as_str().unwrap_or_default(),
                        params["fromCheckpointId"].as_str().unwrap_or_default(),
                        params["toCheckpointId"].as_str().unwrap_or_default(),
                    )
                    .await
                    .map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Object(json!({
                    "fromCheckpointId": params["fromCheckpointId"],
                    "toCheckpointId": params["toCheckpointId"],
                    "files": files.iter().map(file_json).collect::<Vec<_>>(),
                })))
            })
        })
    };

    let restore = {
        let checkpoints = Arc::clone(&checkpoints);
        handler(move |arg: Wire| {
            let params = first_object(arg);
            let checkpoints = Arc::clone(&checkpoints);
            Box::pin(async move {
                let result = checkpoints
                    .restore(
                        params["workspacePath"].as_str().unwrap_or_default(),
                        params["fromCheckpointId"].as_str().unwrap_or_default(),
                        params["toCheckpointId"].as_str().unwrap_or_default(),
                        params["force"] == Value::Bool(true),
                    )
                    .await
                    .map_err(failed)?;
                // TS 两条分支各只带自己的键：成功带 `restoredPaths`，被冲突挡住带 `conflicts`。
                if result.success {
                    Ok::<Wire, (String, String)>(Wire::Object(json!({
                        "success": true,
                        "restoredPaths": result.restored_paths,
                    })))
                } else {
                    Ok::<Wire, (String, String)>(Wire::Object(json!({
                        "success": false,
                        "conflicts": result
                            .conflicts
                            .iter()
                            .map(conflict_json)
                            .collect::<Vec<_>>(),
                    })))
                }
            })
        })
    };

    let delete = {
        handler(move |arg: Wire| {
            let params = first_object(arg);
            let checkpoints = Arc::clone(&checkpoints);
            Box::pin(async move {
                checkpoints
                    .delete(
                        params["workspacePath"].as_str().unwrap_or_default(),
                        params["checkpointId"].as_str().unwrap_or_default(),
                    )
                    .await
                    .map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Undefined)
            })
        })
    };

    Channel::new(
        "git-checkpoint",
        vec![
            ("createCheckpoint", create),
            ("diffCheckpoints", diff),
            ("restoreBetweenCheckpoints", restore),
            ("deleteCheckpoint", delete),
        ],
    )
}
