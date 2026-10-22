//! 可移植暂存架（TS `svnShelf.ts`）。
//!
//! svn 1.14 已移除实验性的 `svn shelve`，这里用 `svn diff` + 未纳入版本控制
//! patch 生成补丁，保存在 `wcRoot/.svn/escode-shelves/`，应用时走 `svn patch`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::fs;

use super::config::{COMMAND_TIMEOUT, DIFF_TIMEOUT, normalize_svn_path};
use super::errors::failure;
use super::paths::{build_unversioned_patch, is_safe_relative_path, local_target};
use super::reads::{read_changes, read_workspace_info};
use super::{RunRequest, SvnService, text, workspace_key};

const SHELF_NAME_MAX: usize = 64;

fn valid_shelf_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= SHELF_NAME_MAX
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn resolve_shelf_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        return Some(format!("shelf-{millis}"));
    }
    valid_shelf_name(trimmed).then(|| trimmed.to_owned())
}

fn shelf_directory(wc_root: &str) -> PathBuf {
    Path::new(wc_root).join(".svn").join("escode-shelves")
}

fn patch_path(wc_root: &str, name: &str) -> PathBuf {
    shelf_directory(wc_root).join(format!("{name}.patch"))
}

fn metadata_path(wc_root: &str, name: &str) -> PathBuf {
    shelf_directory(wc_root).join(format!("{name}.json"))
}

async fn resolve_wc_root(service: &SvnService, params: &Value) -> Result<String, Value> {
    let workspace = read_workspace_info(service, params).await?;
    Ok(workspace
        .info
        .wc_root
        .unwrap_or_else(|| text(params, "workspacePath")))
}

fn parse_metadata(name: &str, raw: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if value["name"].as_str() != Some(name)
        || !value["message"].is_string()
        || value["createdAt"].as_u64().is_none()
        || !value["paths"].as_array()?.iter().all(Value::is_string)
    {
        return None;
    }
    Some(json!({
        "name": name,
        "message": value["message"],
        "createdAt": value["createdAt"],
        "paths": value["paths"],
    }))
}

pub(crate) async fn list_shelves(service: &SvnService, params: &Value) -> Value {
    let wc_root = match resolve_wc_root(service, params).await {
        Ok(root) => root,
        Err(error) => return error,
    };
    let directory = shelf_directory(&wc_root);
    let Ok(mut entries) = fs::read_dir(&directory).await else {
        return json!({ "ok": true, "shelves": Vec::<Value>::new() });
    };
    let mut shelves = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file_name.strip_suffix(".json") else {
            continue;
        };
        let Ok(raw) = fs::read_to_string(entry.path()).await else {
            continue;
        };
        if let Some(shelf) = parse_metadata(name, &raw) {
            shelves.push(shelf);
        }
    }
    shelves.sort_by(|left, right| {
        right["createdAt"]
            .as_u64()
            .unwrap_or_default()
            .cmp(&left["createdAt"].as_u64().unwrap_or_default())
    });
    json!({ "ok": true, "shelves": shelves })
}

pub(crate) async fn shelve(service: &SvnService, params: &Value) -> Value {
    let mut seen = BTreeSet::new();
    let paths: Vec<String> = super::strings(params, "paths")
        .into_iter()
        .map(|path| normalize_svn_path(&path))
        .filter(|path| seen.insert(path.clone()))
        .collect();
    if paths.is_empty() || !paths.iter().all(|path| is_safe_relative_path(path)) {
        return failure("invalid-request", "paths must be relative to the workspace");
    }
    let Some(name) = resolve_shelf_name(&text(params, "name")) else {
        return failure("invalid-request", "invalid shelf name");
    };
    let changes = read_changes(service, params).await;
    if changes["ok"].as_bool() != Some(true) {
        return changes;
    }
    let mut by_path: BTreeMap<String, Value> = BTreeMap::new();
    for change in changes["changes"].as_array().into_iter().flatten() {
        if let Some(path) = change["path"].as_str() {
            by_path.insert(path.to_owned(), change.clone());
        }
    }
    let selected: Vec<Value> = paths
        .iter()
        .filter_map(|path| by_path.get(path).cloned())
        .collect();
    if selected.len() != paths.len() {
        return failure(
            "invalid-request",
            "selected paths have no changes to shelve",
        );
    }
    for change in &selected {
        let status = change["status"].as_str().unwrap_or_default();
        if change["isBinary"].as_bool() == Some(true) {
            return failure("invalid-request", "binary files cannot be shelved");
        }
        if change["isDirectory"].as_bool() == Some(true) && status == "unversioned" {
            return failure(
                "invalid-request",
                "unversioned directories cannot be shelved",
            );
        }
    }
    let wc_root = match resolve_wc_root(service, params).await {
        Ok(root) => root,
        Err(error) => return error,
    };
    let directory = shelf_directory(&wc_root);
    let patch_file = patch_path(&wc_root, &name);
    let metadata_file = metadata_path(&wc_root, &name);
    if fs::metadata(&patch_file).await.is_ok() {
        return failure("invalid-request", "a shelf with this name already exists");
    }

    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let missing: Vec<String> = selected
                .iter()
                .filter(|change| change["status"].as_str() == Some("missing"))
                .filter_map(|change| change["path"].as_str().map(str::to_owned))
                .collect();
            if !missing.is_empty() {
                let mut args = vec!["delete".to_owned(), "--force".to_owned(), "--".to_owned()];
                args.extend(missing.iter().map(|path| local_target(path)));
                if let Err(error) = service
                    .run(RunRequest::new(&text(params, "workspacePath"), args))
                    .await
                {
                    return error;
                }
            }
            // `svn diff` / `revert` 不接受未纳入版本控制的路径。
            let versioned: Vec<String> = paths
                .iter()
                .filter(|path| {
                    by_path
                        .get(*path)
                        .and_then(|change| change["status"].as_str())
                        != Some("unversioned")
                })
                .cloned()
                .collect();
            let mut patches: Vec<String> = Vec::new();
            if !versioned.is_empty() {
                let mut args = vec![
                    "diff".to_owned(),
                    "--internal-diff".to_owned(),
                    "--depth".to_owned(),
                    "infinity".to_owned(),
                    "--".to_owned(),
                ];
                args.extend(versioned.iter().map(|path| local_target(path)));
                match service
                    .run(
                        RunRequest::new(&text(params, "workspacePath"), args).timeout(DIFF_TIMEOUT),
                    )
                    .await
                {
                    Ok(output) => {
                        let text = output.stdout.trim_end();
                        if !text.is_empty() {
                            patches.push(text.to_owned());
                        }
                    }
                    Err(error) => {
                        if !missing.is_empty() {
                            let mut revert = vec![
                                "revert".to_owned(),
                                "--depth".to_owned(),
                                "infinity".to_owned(),
                                "--".to_owned(),
                            ];
                            revert.extend(missing.iter().map(|path| local_target(path)));
                            let _ = service
                                .run(RunRequest::new(&text(params, "workspacePath"), revert))
                                .await;
                        }
                        return error;
                    }
                }
            }
            for change in &selected {
                if change["status"].as_str() != Some("unversioned") {
                    continue;
                }
                let path = change["path"].as_str().unwrap_or_default();
                let absolute = Path::new(&text(params, "workspacePath")).join(path);
                let synthetic = build_unversioned_patch(&absolute.to_string_lossy(), path).await;
                if synthetic.is_binary {
                    return failure("invalid-request", "binary files cannot be shelved");
                }
                let Some(patch) = synthetic.patch else {
                    return failure("invalid-request", "unversioned file is too large to shelve");
                };
                if synthetic.truncated {
                    return failure("invalid-request", "unversioned file is too large to shelve");
                }
                patches.push(patch);
            }
            let patch = patches.join("\n");
            if patch.trim().is_empty() {
                return failure("invalid-request", "there are no changes to shelve");
            }
            let created_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let metadata = json!({
                "version": 1,
                "name": name.clone(),
                "message": text(params, "message").trim(),
                "createdAt": created_at,
                "paths": paths.clone(),
            });
            if fs::create_dir_all(&directory).await.is_err()
                || fs::write(&patch_file, &patch).await.is_err()
                || fs::write(&metadata_file, metadata.to_string())
                    .await
                    .is_err()
            {
                return failure("unknown", "failed to write shelf");
            }
            let mut revert = vec![
                "revert".to_owned(),
                "--depth".to_owned(),
                "infinity".to_owned(),
                "--".to_owned(),
            ];
            revert.extend(versioned.iter().map(|path| local_target(path)));
            let reverted = if versioned.is_empty() {
                Ok(())
            } else {
                service
                    .run(
                        RunRequest::new(&text(params, "workspacePath"), revert)
                            .timeout(COMMAND_TIMEOUT),
                    )
                    .await
                    .map(|_| ())
            };
            if let Err(error) = reverted {
                let _ = fs::remove_file(&patch_file).await;
                let _ = fs::remove_file(&metadata_file).await;
                return error;
            }
            for change in &selected {
                let status = change["status"].as_str().unwrap_or_default();
                if status == "added" || status == "unversioned" {
                    let path = change["path"].as_str().unwrap_or_default();
                    let absolute = Path::new(&text(params, "workspacePath")).join(path);
                    if fs::metadata(&absolute)
                        .await
                        .is_ok_and(|meta| meta.is_dir())
                    {
                        let _ = fs::remove_dir_all(&absolute).await;
                    } else {
                        let _ = fs::remove_file(&absolute).await;
                    }
                }
            }
            json!({
                "ok": true,
                "shelf": {
                    "name": name,
                    "message": text(params, "message").trim(),
                    "createdAt": created_at,
                    "paths": paths,
                },
            })
        })
        .await
}

pub(crate) async fn unshelve(service: &SvnService, params: &Value) -> Value {
    let Some(name) = resolve_shelf_name(&text(params, "name")) else {
        return failure("invalid-request", "invalid shelf name");
    };
    let wc_root = match resolve_wc_root(service, params).await {
        Ok(root) => root,
        Err(error) => return error,
    };
    let patch_file = patch_path(&wc_root, &name);
    if fs::metadata(&patch_file).await.is_err() {
        return failure("shelf-not-found", "shelf was not found");
    }
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            match service
                .run(
                    RunRequest::new(
                        &text(params, "workspacePath"),
                        vec![
                            "patch".to_owned(),
                            "--".to_owned(),
                            patch_file.to_string_lossy().into_owned(),
                        ],
                    )
                    .timeout(DIFF_TIMEOUT),
                )
                .await
            {
                Ok(_) => json!({ "ok": true }),
                Err(error) => error,
            }
        })
        .await
}

pub(crate) async fn delete_shelf(service: &SvnService, params: &Value) -> Value {
    let Some(name) = resolve_shelf_name(&text(params, "name")) else {
        return failure("invalid-request", "invalid shelf name");
    };
    let wc_root = match resolve_wc_root(service, params).await {
        Ok(root) => root,
        Err(error) => return error,
    };
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let mut found = false;
            for file in [patch_path(&wc_root, &name), metadata_path(&wc_root, &name)] {
                if fs::metadata(&file).await.is_ok() {
                    found = true;
                }
                let _ = fs::remove_file(&file).await;
            }
            if found {
                json!({ "ok": true })
            } else {
                failure("shelf-not-found", "shelf was not found")
            }
        })
        .await
}
