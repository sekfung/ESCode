//! svn 写操作（TS `svn/svnWrites.ts`）。所有写操作按 workspace 身份 key 串行。

use std::collections::BTreeSet;

use escode_host_control::logging;
use serde_json::{Value, json};

use super::config::{CLEANUP_TIMEOUT, LONG_WRITE_TIMEOUT, normalize_svn_path};
use super::errors::failure;
use super::parse::{count_updated_paths, item_status, parse_committed_revision};
use super::paths::{encode_segment, is_safe_relative_path, local_target};
use super::reads::{count_conflicts, read_info, read_status, read_workspace_info};
use super::{RunRequest, SvnService, credentials, text, workspace_key};

pub(crate) async fn commit(service: &SvnService, params: &Value) -> Value {
    let mut seen = BTreeSet::new();
    let paths: Vec<String> = super::strings(params, "paths")
        .into_iter()
        .map(|path| normalize_svn_path(&path))
        .filter(|path| seen.insert(path.clone()))
        .collect();
    if paths.is_empty() {
        return failure("nothing-to-commit", "no paths selected");
    }
    let message = text(params, "message");
    if message.trim().is_empty() {
        return failure("invalid-request", "commit message is empty");
    }
    if !paths.iter().all(|path| is_safe_relative_path(path)) {
        return failure("invalid-request", "paths must be relative to the workspace");
    }
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || commit_in_queue(service, params, paths, message))
        .await
}

async fn commit_in_queue(
    service: &SvnService,
    params: &Value,
    paths: Vec<String>,
    message: String,
) -> Value {
    let workspace_path = text(params, "workspacePath");
    let status = match read_status(
        service,
        &workspace_path,
        &paths
            .iter()
            .map(|path| local_target(path))
            .collect::<Vec<_>>(),
    )
    .await
    {
        Ok(status) => status,
        Err(error) => return error,
    };
    let by_path: std::collections::HashMap<&str, &super::StatusEntry> = status
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    if paths.iter().any(|path| {
        by_path
            .get(path.as_str())
            .is_some_and(|entry| item_status(entry) == Some("conflicted"))
    }) {
        return failure(
            "unresolved-conflicts",
            "selected paths contain unresolved conflicts",
        );
    }
    let to_add: Vec<&String> = paths
        .iter()
        .filter(|path| {
            by_path
                .get(path.as_str())
                .is_some_and(|entry| entry.item == "unversioned")
        })
        .collect();
    let to_delete: Vec<&String> = paths
        .iter()
        .filter(|path| {
            by_path
                .get(path.as_str())
                .is_some_and(|entry| entry.item == "missing")
        })
        .collect();
    if !to_add.is_empty() {
        let mut args = vec![
            "add".to_owned(),
            "--parents".to_owned(),
            "--depth".to_owned(),
            "infinity".to_owned(),
            "--".to_owned(),
        ];
        args.extend(to_add.iter().map(|path| local_target(path)));
        if let Err(error) = service.run(RunRequest::new(&workspace_path, args)).await {
            return error;
        }
    }
    if !to_delete.is_empty() {
        let mut args = vec!["delete".to_owned(), "--".to_owned()];
        args.extend(to_delete.iter().map(|path| local_target(path)));
        if let Err(error) = service.run(RunRequest::new(&workspace_path, args)).await {
            return error;
        }
    }
    // --depth empty 精确提交勾选项；新加入的目录展开其子项一起提交。
    let mut targets: BTreeSet<String> = paths.iter().cloned().collect();
    for path in &to_add {
        if let Ok(children) = read_status(service, &workspace_path, &[local_target(path)]).await {
            for child in children {
                if child.item == "added" {
                    targets.insert(child.path);
                }
            }
        }
    }
    let message_file = match write_message_file(service, &message).await {
        Ok(file) => file,
        Err(error) => return failure("unknown", format!("{error:#}")),
    };
    let mut args = vec![
        "commit".to_owned(),
        "--depth".to_owned(),
        "empty".to_owned(),
        "-F".to_owned(),
        message_file.0.to_string_lossy().into_owned(),
        "--encoding".to_owned(),
        "UTF-8".to_owned(),
        "--".to_owned(),
    ];
    args.extend(targets.iter().map(|path| local_target(path)));
    let outcome = service
        .run(
            RunRequest::new(&workspace_path, args)
                .credentials(credentials(params))
                .timeout(LONG_WRITE_TIMEOUT),
        )
        .await;
    let _ = tokio::fs::remove_dir_all(&message_file.1).await;
    let output = match outcome {
        Ok(output) => output,
        Err(error) => return error,
    };
    let Some(revision) = parse_committed_revision(&output.stdout) else {
        return failure("nothing-to-commit", "svn reported nothing to commit");
    };
    logging::info(
        "svn",
        format!(
            "svn 提交完成 workspaceKey={} revision={revision} fileCount={}",
            workspace_key(params),
            targets.len()
        ),
    );
    json!({
        "ok": true,
        "revision": revision,
        "addedPaths": to_add.iter().map(|path| (**path).clone()).collect::<Vec<_>>(),
    })
}

/// 返回 (message.txt, 临时目录)；调用方在命令结束后删除目录。
async fn write_message_file(
    service: &SvnService,
    message: &str,
) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf)> {
    let dir = service
        .temp_root()
        .join(format!("escode-svn-msg-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("message.txt");
    tokio::fs::write(&path, message).await?;
    Ok((path, dir))
}

pub(crate) async fn update(service: &SvnService, params: &Value) -> Value {
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let workspace_path = text(params, "workspacePath");
            // 固定 --accept postpone：冲突留给界面处理，不依赖不同 svn 版本的非交互默认值。
            let args = vec![
                "update".to_owned(),
                "--accept".to_owned(),
                "postpone".to_owned(),
                "--".to_owned(),
                ".".to_owned(),
            ];
            let output = match service
                .run(
                    RunRequest::new(&workspace_path, args)
                        .credentials(credentials(params))
                        .timeout(LONG_WRITE_TIMEOUT),
                )
                .await
            {
                Ok(output) => output,
                Err(error) => return error,
            };
            let revision = read_info(service, &workspace_path, &[".".to_owned()], None)
                .await
                .ok()
                .and_then(|entries| entries.first().and_then(|entry| entry.revision))
                .unwrap_or(0);
            let conflict_count = count_conflicts(service, &workspace_path).await;
            logging::info(
                "svn",
                format!(
                    "svn 更新完成 workspaceKey={key} revision={revision} conflictCount={conflict_count}"
                ),
            );
            json!({
                "ok": true,
                "revision": revision,
                "changedFileCount": count_updated_paths(&output.stdout),
                "conflictCount": conflict_count,
            })
        })
        .await
}

pub(crate) async fn revert(service: &SvnService, params: &Value) -> Value {
    let mut seen = BTreeSet::new();
    let paths: Vec<String> = super::strings(params, "paths")
        .into_iter()
        .map(|path| normalize_svn_path(&path))
        .filter(|path| seen.insert(path.clone()))
        .collect();
    if paths.is_empty() || !paths.iter().all(|path| is_safe_relative_path(path)) {
        return failure("invalid-request", "paths must be relative to the workspace");
    }
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            // 不加 --remove-added：撤销 add 后文件保留在磁盘上，与界面“还原”的确认文案一致。
            let mut args = vec![
                "revert".to_owned(),
                "--depth".to_owned(),
                "infinity".to_owned(),
                "--".to_owned(),
            ];
            args.extend(paths.iter().map(|path| local_target(path)));
            match service
                .run(RunRequest::new(&text(params, "workspacePath"), args))
                .await
            {
                Ok(_) => json!({ "ok": true }),
                Err(error) => error,
            }
        })
        .await
}

const ACCEPT_VALUES: [&str; 3] = ["mine-full", "theirs-full", "working"];

pub(crate) async fn resolve(service: &SvnService, params: &Value) -> Value {
    let path = text(params, "path");
    let accept = text(params, "accept");
    if !is_safe_relative_path(&path) {
        return failure("invalid-request", "path must be relative to the workspace");
    }
    if !ACCEPT_VALUES.contains(&accept.as_str()) {
        return failure("invalid-request", "invalid accept mode");
    }
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let args = vec![
                "resolve".to_owned(),
                "--accept".to_owned(),
                accept,
                "--".to_owned(),
                local_target(&path),
            ];
            match service
                .run(RunRequest::new(&text(params, "workspacePath"), args))
                .await
            {
                Ok(_) => json!({ "ok": true }),
                Err(error) => error,
            }
        })
        .await
}

pub(crate) async fn switch_branch(service: &SvnService, params: &Value) -> Value {
    let relative_path = text(params, "relativePath")
        .trim_start_matches('^')
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned();
    if relative_path.is_empty()
        || relative_path.split('/').any(|segment| segment == "..")
        || relative_path.contains(['\r', '\n'])
    {
        return failure("invalid-request", "invalid branch path");
    }
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let workspace = match read_workspace_info(service, params).await {
                Ok(workspace) => workspace,
                Err(error) => return error,
            };
            let (Some(wc_root), Some(repository_root)) = (
                workspace.info.wc_root.clone(),
                workspace.info.repository_root.clone(),
            ) else {
                return failure("invalid-request", "working copy root is not a branch root");
            };
            if !workspace.can_switch {
                return failure("invalid-request", "working copy root is not a branch root");
            }
            let encoded = relative_path
                .split('/')
                .map(encode_segment)
                .collect::<Vec<_>>()
                .join("/");
            let target_url = format!("{repository_root}/{encoded}");
            let args = vec![
                "switch".to_owned(),
                "--accept".to_owned(),
                "postpone".to_owned(),
                "--".to_owned(),
                format!("{target_url}@HEAD"),
                ".".to_owned(),
            ];
            if let Err(error) = service
                .run(
                    RunRequest::new(&wc_root, args)
                        .credentials(credentials(params))
                        .timeout(LONG_WRITE_TIMEOUT),
                )
                .await
            {
                return error;
            }
            let workspace_path = text(params, "workspacePath");
            let after = read_workspace_info(service, params).await.ok();
            let conflict_count = count_conflicts(service, &workspace_path).await;
            logging::info(
                "svn",
                format!(
                    "svn 切换完成 workspaceKey={key} branchKind={} conflictCount={conflict_count}",
                    after
                        .as_ref()
                        .and_then(|workspace| workspace.branch.as_ref())
                        .map_or("", |branch| branch.kind)
                ),
            );
            let mut result = json!({
                "ok": true,
                "revision": after
                    .as_ref()
                    .and_then(|workspace| workspace.info.revision)
                    .unwrap_or(0),
                "conflictCount": conflict_count,
            });
            if let Some(branch) = after.and_then(|workspace| workspace.branch) {
                result["branch"] = branch.to_json();
            }
            result
        })
        .await
}

pub(crate) async fn cleanup(service: &SvnService, params: &Value) -> Value {
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let workspace_path = text(params, "workspacePath");
            let wc_root = read_info(service, &workspace_path, &[".".to_owned()], None)
                .await
                .ok()
                .and_then(|entries| entries.first().and_then(|entry| entry.wc_root.clone()))
                .unwrap_or_else(|| workspace_path.clone());
            let args = vec!["cleanup".to_owned(), "--".to_owned(), ".".to_owned()];
            let result = service
                .run(RunRequest::new(&wc_root, args).timeout(CLEANUP_TIMEOUT))
                .await;
            match result {
                Ok(_) => {
                    logging::info("svn", format!("svn 清理完成 workspaceKey={key}"));
                    json!({ "ok": true })
                }
                Err(error) => error,
            }
        })
        .await
}
