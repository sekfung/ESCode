//! svn 读面：info / status / 改动列表 / diff（TS `svn/svnReads.ts`）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::{Map, Value, json};

use super::config::{
    DIFF_BYTES, DIFF_TIMEOUT, LINE_STAT_MAX_FILES, REMOTE_READ_TIMEOUT, normalize_svn_path,
};
use super::errors::failure;
use super::parse::{self, InfoEntry, StatusEntry};
use super::paths::{
    BranchRef, build_unversioned_patch, derive_branch_ref, is_directory, is_safe_relative_path,
    local_target, read_unversioned_file_stat, relative_path, relative_repository_path,
    split_diff_by_index, to_workspace_relative,
};
use super::{Credentials, RunRequest, SvnService, text};

pub(crate) struct WorkspaceInfo {
    pub info: InfoEntry,
    pub branch: Option<BranchRef>,
    pub can_switch: bool,
}

pub(crate) async fn read_info(
    service: &SvnService,
    cwd: &str,
    targets: &[String],
    credentials: Option<Credentials>,
) -> Result<Vec<InfoEntry>, Value> {
    let mut args = vec!["info".to_owned(), "--xml".to_owned(), "--".to_owned()];
    args.extend(targets.iter().cloned());
    let output = service
        .run(RunRequest::new(cwd, args).credentials(credentials))
        .await?;
    Ok(parse::parse_info(&output.stdout))
}

pub(crate) async fn read_status(
    service: &SvnService,
    workspace_path: &str,
    targets: &[String],
) -> Result<Vec<StatusEntry>, Value> {
    let mut args = vec![
        "status".to_owned(),
        "--xml".to_owned(),
        "--no-ignore".to_owned(),
        "--".to_owned(),
    ];
    args.extend(targets.iter().cloned());
    let output = service.run(RunRequest::new(workspace_path, args)).await?;
    Ok(parse::parse_status(&output.stdout))
}

/// 工作副本当前冲突数（update / switch / merge 完成后统计）。
pub(crate) async fn count_conflicts(service: &SvnService, workspace_path: &str) -> u64 {
    match read_status(service, workspace_path, &[".".to_owned()]).await {
        Ok(entries) => entries
            .iter()
            .filter(|entry| parse::item_status(entry) == Some("conflicted"))
            .count() as u64,
        Err(_) => 0,
    }
}

pub(crate) async fn read_workspace_info(
    service: &SvnService,
    params: &Value,
) -> Result<WorkspaceInfo, Value> {
    let workspace_path = text(params, "workspacePath");
    let entries = read_info(service, &workspace_path, &[".".to_owned()], None).await?;
    let Some(info) = entries.into_iter().next() else {
        return Err(failure("not-working-copy", "not a working copy"));
    };
    let relative = match (&info.url, &info.repository_root) {
        (Some(url), Some(root)) => relative_repository_path(url, root),
        _ => None,
    };
    let branch = relative.as_deref().map(derive_branch_ref);
    let mut can_switch = false;
    if let (Some(branch), Some(relative_url), Some(wc_root)) = (
        branch.as_ref(),
        relative.as_deref(),
        info.wc_root.as_deref(),
    ) && branch.kind != "other"
    {
        let workspace_in_wc = relative_path(wc_root, &workspace_path);
        let wc_root_relative_url = if !workspace_in_wc.is_empty() && workspace_in_wc != "." {
            strip_suffix_segment(relative_url, &workspace_in_wc)
        } else {
            relative_url.to_owned()
        };
        can_switch = wc_root_relative_url == branch.relative_path;
    }
    Ok(WorkspaceInfo {
        info,
        branch,
        can_switch,
    })
}

fn strip_suffix_segment(value: &str, suffix: &str) -> String {
    let combined = format!("/{suffix}");
    value.strip_suffix(&combined).unwrap_or(value).to_owned()
}

const DIFFABLE: [&str; 5] = ["modified", "added", "deleted", "replaced", "conflicted"];

pub(crate) async fn read_changes(service: &SvnService, params: &Value) -> Value {
    let workspace_path = text(params, "workspacePath");
    let status = match read_status(service, &workspace_path, &[".".to_owned()]).await {
        Ok(status) => status,
        Err(error) => return error,
    };
    let mut ignored_paths: Vec<String> = Vec::new();
    let mut visible: Vec<(StatusEntry, &'static str)> = Vec::new();
    for entry in status {
        if entry.item == "ignored" {
            ignored_paths.push(entry.path);
            continue;
        }
        if entry.in_external {
            continue;
        }
        if let Some(item_status) = parse::item_status(&entry) {
            visible.push((entry, item_status));
        }
    }
    let conflicted_paths: Vec<String> = visible
        .iter()
        .filter(|(_, status)| *status == "conflicted")
        .map(|(entry, _)| entry.path.clone())
        .collect();
    let conflict_details =
        collect_conflict_details(service, &workspace_path, &conflicted_paths).await;
    let artifact_paths: BTreeSet<String> = conflict_details
        .values()
        .flat_map(|entry| {
            entry
                .conflict
                .as_ref()
                .map(|conflict| {
                    conflict
                        .artifact_paths
                        .iter()
                        .map(|path| to_workspace_relative(&workspace_path, path))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();
    let conflicted_set: BTreeSet<&str> = conflicted_paths.iter().map(String::as_str).collect();
    // 冲突辅助文件（.mine / .rN）是 svn 生成的临时文件，不作为未纳入版本控制的改动展示。
    let filtered: Vec<(StatusEntry, &'static str)> = visible
        .into_iter()
        .filter(|(entry, status)| {
            if *status != "unversioned" {
                return true;
            }
            if artifact_paths.contains(&entry.path) {
                return false;
            }
            let base = strip_conflict_suffix(&entry.path);
            base == entry.path || !conflicted_set.contains(base.as_str())
        })
        .collect();
    let line_stats = collect_line_stats(
        service,
        &workspace_path,
        &filtered
            .iter()
            .filter(|(_, status)| DIFFABLE.contains(status))
            .map(|(entry, _)| entry.path.clone())
            .collect::<Vec<_>>(),
    )
    .await;

    let mut changes = Vec::with_capacity(filtered.len());
    for (entry, status) in filtered {
        let absolute = Path::new(&workspace_path).join(&entry.path);
        let detail = conflict_details.get(&entry.path);
        let mut change = Map::new();
        change.insert("path".to_owned(), Value::String(entry.path.clone()));
        change.insert("status".to_owned(), Value::String(status.to_owned()));
        change.insert(
            "propsModified".to_owned(),
            Value::Bool(entry.props == "modified" || entry.props == "conflicted"),
        );
        if status == "conflicted" {
            let kinds: Vec<&str> = detail
                .and_then(|entry| entry.conflict.as_ref())
                .map(|conflict| conflict.kinds.clone())
                .unwrap_or_else(|| {
                    vec![if entry.tree_conflicted {
                        "tree"
                    } else {
                        "text"
                    }]
                });
            let mut conflict = json!({ "kinds": kinds });
            if let Some(description) =
                describe_tree_conflict(detail.and_then(|entry| entry.conflict.as_ref()))
            {
                conflict["description"] = Value::String(description);
            }
            change.insert("conflict".to_owned(), conflict);
        }
        if status == "unversioned" {
            let stat = read_unversioned_file_stat(&absolute.to_string_lossy()).await;
            change.insert("isDirectory".to_owned(), Value::Bool(stat.is_directory));
            change.insert("additions".to_owned(), Value::from(stat.additions));
            change.insert("deletions".to_owned(), Value::from(0));
            change.insert("isBinary".to_owned(), Value::Bool(stat.is_binary));
        } else {
            let stat = line_stats.get(&entry.path).copied().unwrap_or_default();
            change.insert(
                "isDirectory".to_owned(),
                Value::Bool(is_directory(&absolute.to_string_lossy()).await),
            );
            change.insert("additions".to_owned(), Value::from(stat.additions));
            change.insert("deletions".to_owned(), Value::from(stat.deletions));
            change.insert("isBinary".to_owned(), Value::Bool(stat.is_binary));
        }
        changes.push(Value::Object(change));
    }
    json!({ "ok": true, "changes": changes, "ignoredPaths": ignored_paths })
}

fn strip_conflict_suffix(path: &str) -> String {
    for suffix in [".mine", ".working"] {
        if let Some(base) = path.strip_suffix(suffix) {
            return base.to_owned();
        }
    }
    // `.rN` / `.merge-left.rN` / `.merge-right.rN`：去掉末尾的 `.rN` 后再看 merge 后缀。
    if let Some((base, tail)) = path.rsplit_once(".r")
        && !tail.is_empty()
        && tail.chars().all(|ch| ch.is_ascii_digit())
    {
        for suffix in [".merge-left", ".merge-right"] {
            if let Some(root) = base.strip_suffix(suffix) {
                return root.to_owned();
            }
        }
        return base.to_owned();
    }
    path.to_owned()
}

async fn collect_conflict_details(
    service: &SvnService,
    workspace_path: &str,
    paths: &[String],
) -> HashMap<String, InfoEntry> {
    if paths.is_empty() {
        return HashMap::new();
    }
    let targets: Vec<String> = paths.iter().map(|path| local_target(path)).collect();
    let Ok(entries) = read_info(service, workspace_path, &targets, None).await else {
        return HashMap::new();
    };
    entries
        .into_iter()
        .map(|entry| (to_workspace_relative(workspace_path, &entry.path), entry))
        .collect()
}

fn describe_tree_conflict(conflict: Option<&parse::InfoConflict>) -> Option<String> {
    let tree = conflict?.tree_conflict.as_ref()?;
    Some(format!(
        "local {}, incoming {} upon {}",
        tree.reason.as_deref().unwrap_or("change"),
        tree.action.as_deref().unwrap_or("change"),
        tree.operation.as_deref().unwrap_or("update"),
    ))
}

async fn collect_line_stats(
    service: &SvnService,
    workspace_path: &str,
    paths: &[String],
) -> BTreeMap<String, parse::DiffCount> {
    let mut stats = BTreeMap::new();
    if paths.is_empty() || paths.len() > LINE_STAT_MAX_FILES {
        return stats;
    }
    let mut args = vec![
        "diff".to_owned(),
        "--internal-diff".to_owned(),
        "--depth".to_owned(),
        "empty".to_owned(),
        "--".to_owned(),
    ];
    args.extend(paths.iter().map(|path| local_target(path)));
    let Ok(output) = service
        .run(RunRequest::new(workspace_path, args).timeout(DIFF_TIMEOUT))
        .await
    else {
        return stats;
    };
    for (path, section) in split_diff_by_index(&output.stdout) {
        stats.insert(path, parse::count_diff_lines(&section));
    }
    stats
}

pub(crate) async fn read_diff(service: &SvnService, params: &Value) -> Value {
    let workspace_path = text(params, "workspacePath");
    let path = text(params, "path");
    if let Some(revision) = params.get("revision").and_then(Value::as_i64) {
        let repo_path = if path.starts_with('/') {
            path.clone()
        } else {
            format!("/{path}")
        };
        let args = vec![
            "diff".to_owned(),
            "--internal-diff".to_owned(),
            "-c".to_owned(),
            revision.to_string(),
            "--".to_owned(),
            format!("^{repo_path}@{revision}"),
        ];
        return match service
            .run(
                RunRequest::new(&workspace_path, args)
                    .timeout(REMOTE_READ_TIMEOUT)
                    .max_output(DIFF_BYTES),
            )
            .await
        {
            Ok(output) => diff_result(&path, &output.stdout, output.output_truncated),
            Err(error) => error,
        };
    }
    if !is_safe_relative_path(&path) {
        return failure("invalid-request", "path must be relative to the workspace");
    }
    let status = match read_status(service, &workspace_path, &[local_target(&path)]).await {
        Ok(status) => status,
        Err(error) => return error,
    };
    let entry = status
        .iter()
        .find(|entry| entry.path == normalize_svn_path(&path));
    if entry.is_some_and(|entry| entry.item == "unversioned") {
        let absolute = Path::new(&workspace_path).join(&path);
        if is_directory(&absolute.to_string_lossy()).await {
            return json!({
                "ok": true,
                "path": path,
                "patch": Value::Null,
                "isBinary": false,
                "truncated": false,
            });
        }
        let patched = build_unversioned_patch(&absolute.to_string_lossy(), &path).await;
        return json!({
            "ok": true,
            "path": path,
            "patch": patched.patch,
            "isBinary": patched.is_binary,
            "truncated": patched.truncated,
        });
    }
    let args = vec![
        "diff".to_owned(),
        "--internal-diff".to_owned(),
        "--depth".to_owned(),
        "empty".to_owned(),
        "--".to_owned(),
        local_target(&path),
    ];
    match service
        .run(
            RunRequest::new(&workspace_path, args)
                .timeout(DIFF_TIMEOUT)
                .max_output(DIFF_BYTES),
        )
        .await
    {
        Ok(output) => diff_result(&path, &output.stdout, output.output_truncated),
        Err(error) => error,
    }
}

fn diff_result(path: &str, stdout: &str, truncated: bool) -> Value {
    json!({
        "ok": true,
        "path": path,
        "patch": if stdout.is_empty() { Value::Null } else { Value::String(stdout.to_owned()) },
        "isBinary": parse::count_diff_lines(stdout).is_binary,
        "truncated": truncated,
    })
}
