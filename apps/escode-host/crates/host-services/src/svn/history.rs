//! svn 历史与服务器侧读取（TS `svn/svnHistory.ts`）。
//!
//! 修订不可变，因此以数字起点读取的历史页可安全缓存；从 HEAD 读取的首页不缓存。

use serde_json::{Value, json};
use std::collections::BTreeSet;

use super::config::{DEFAULT_LOG_LIMIT, MAX_BLAME_LINES, MAX_LOG_LIMIT, REMOTE_READ_TIMEOUT};
use super::errors::failure;
use super::parse::{LogEntry, parse_blame, parse_list, parse_log};
use super::paths::{
    derive_branch_ref, is_safe_relative_path, local_target, relative_repository_path,
};
use super::reads::{read_info, read_workspace_info};
use super::xml::{child, children, int_attr};
use super::{RunRequest, SvnService, credentials, text};

fn workspace_url(workspace: &super::reads::WorkspaceInfo) -> Result<String, Value> {
    workspace
        .info
        .url
        .clone()
        .ok_or_else(|| failure("not-working-copy", "working copy URL is unavailable"))
}

pub(crate) async fn get_log(service: &SvnService, params: &Value) -> Value {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LOG_LIMIT, |value| value as usize)
        .clamp(1, MAX_LOG_LIMIT);
    let workspace = match read_workspace_info(service, params).await {
        Ok(workspace) => workspace,
        Err(error) => return error,
    };
    let url = match workspace_url(&workspace) {
        Ok(url) => url,
        Err(error) => return error,
    };
    let start = params.get("startRevision").and_then(Value::as_i64);
    let cache_key = start.map(|start| {
        format!(
            "{}|{}|{}|{}",
            workspace
                .info
                .repository_uuid
                .as_deref()
                .unwrap_or_default(),
            url,
            start,
            limit
        )
    });
    if let Some(key) = &cache_key
        && let Some(cached) = service.cached_log(key).await
    {
        return json!({ "ok": true, "entries": cached["entries"], "hasMore": cached["hasMore"] });
    }
    let revision = start.map_or_else(|| "HEAD".to_owned(), |value| value.to_string());
    let args = vec![
        "log".to_owned(),
        "--xml".to_owned(),
        "-v".to_owned(),
        "--limit".to_owned(),
        (limit + 1).to_string(),
        "-r".to_owned(),
        format!("{revision}:1"),
        "--".to_owned(),
        format!("{url}@{revision}"),
    ];
    let output = match service
        .run(
            RunRequest::new(&text(params, "workspacePath"), args)
                .credentials(credentials(params))
                .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let entries = parse_log(&output.stdout);
    let has_more = entries.len() > limit;
    let entries: Vec<Value> = entries.iter().take(limit).map(LogEntry::to_json).collect();
    let page = json!({ "entries": entries, "hasMore": has_more });
    if let Some(key) = cache_key {
        service.cache_log(key, page.clone()).await;
    }
    json!({ "ok": true, "entries": page["entries"], "hasMore": has_more })
}

pub(crate) async fn get_file_log(service: &SvnService, params: &Value) -> Value {
    let workspace_path = text(params, "workspacePath");
    let path = text(params, "path");
    if !is_safe_relative_path(&path) {
        return failure("invalid-request", "path must be relative to the workspace");
    }
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LOG_LIMIT, |value| value as usize)
        .clamp(1, MAX_LOG_LIMIT);
    let info = match read_info(
        service,
        &workspace_path,
        &[local_target(&path)],
        credentials(params),
    )
    .await
    {
        Ok(entries) => entries,
        Err(error) => {
            // `svn info` 是本地读取；未纳入版本控制时不同 svn 版本的错误码不统一
            //（E155010 / E200009），除明确的进程级失败外统一按 invalid-request 呈现。
            return if matches!(
                error["issue"].as_str(),
                Some("auth-required" | "working-copy-locked" | "svn-unavailable" | "timeout")
            ) {
                error
            } else {
                failure("invalid-request", "path is not versioned")
            };
        }
    };
    let Some(entry) = info.into_iter().next() else {
        return failure("invalid-request", "path is not versioned");
    };
    let (Some(url), Some(root)) = (entry.url.as_deref(), entry.repository_root.as_deref()) else {
        return failure("invalid-request", "path is not versioned");
    };
    let Some(relative) = relative_repository_path(url, root) else {
        return failure("invalid-request", "path is outside the repository");
    };
    let repository_path = format!("/{relative}");
    let start = params.get("startRevision").and_then(Value::as_i64);
    let cache_key = start.map(|start| {
        format!(
            "file|{}|{}|{}|{}",
            entry.repository_uuid.as_deref().unwrap_or_default(),
            url,
            start,
            limit
        )
    });
    if let Some(key) = &cache_key
        && let Some(cached) = service.cached_log(key).await
    {
        return json!({
            "ok": true,
            "entries": cached["entries"],
            "hasMore": cached["hasMore"],
            "repositoryPath": repository_path,
        });
    }
    let revision = start.map_or_else(|| "HEAD".to_owned(), |value| value.to_string());
    let args = vec![
        "log".to_owned(),
        "--xml".to_owned(),
        "-v".to_owned(),
        "--limit".to_owned(),
        (limit + 1).to_string(),
        "-r".to_owned(),
        format!("{revision}:1"),
        "--".to_owned(),
        format!("{url}@{revision}"),
    ];
    let output = match service
        .run(
            RunRequest::new(&workspace_path, args)
                .credentials(credentials(params))
                .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let entries = parse_log(&output.stdout);
    let has_more = entries.len() > limit;
    let entries: Vec<Value> = entries.iter().take(limit).map(LogEntry::to_json).collect();
    let page = json!({ "entries": entries, "hasMore": has_more });
    if let Some(key) = cache_key {
        service.cache_log(key, page.clone()).await;
    }
    json!({
        "ok": true,
        "entries": page["entries"],
        "hasMore": has_more,
        "repositoryPath": repository_path,
    })
}

pub(crate) async fn get_blame(service: &SvnService, params: &Value) -> Value {
    let workspace_path = text(params, "workspacePath");
    let path = text(params, "path");
    if !is_safe_relative_path(&path) {
        return failure("invalid-request", "path must be relative to the workspace");
    }
    let revision = params.get("revision").and_then(Value::as_i64);
    let revision_arg = revision.map_or_else(|| "BASE".to_owned(), |value| value.to_string());
    let mut args = vec!["blame".to_owned(), "--xml".to_owned()];
    if revision.is_some() {
        args.push("-r".to_owned());
        args.push(revision_arg.clone());
    }
    args.push("--".to_owned());
    args.push(local_target(&path));
    let output = match service
        .run(
            RunRequest::new(&workspace_path, args)
                .credentials(credentials(params))
                .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let (mut lines, truncated) = parse_blame(&output.stdout, MAX_BLAME_LINES);
    // svn 1.14 的 `blame --xml` 只给修订/作者，不包含行文本；从同一修订的 cat
    // 输出取内容，保证与 blame 的 BASE（或指定修订）一致。
    let content = match service
        .run(
            RunRequest::new(
                &workspace_path,
                vec![
                    "cat".to_owned(),
                    "-r".to_owned(),
                    revision_arg,
                    "--".to_owned(),
                    local_target(&path),
                ],
            )
            .credentials(credentials(params))
            .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let content_lines: Vec<&str> = content.stdout.lines().collect();
    for (index, line) in lines.iter_mut().enumerate() {
        line.content = content_lines
            .get(index)
            .copied()
            .unwrap_or_default()
            .to_owned();
    }
    json!({
        "ok": true,
        "path": path,
        "lines": lines.iter().map(|line| json!({
            "lineNumber": line.line_number,
            "revision": line.revision,
            "author": line.author,
            "content": line.content,
        })).collect::<Vec<_>>(),
        "truncated": truncated,
    })
}

pub(crate) async fn get_remote_status(service: &SvnService, params: &Value) -> Value {
    // 用 status -u 的过期条目判断，而不是比较修订号：自己刚提交的修订不算“新修订”。
    let args = vec![
        "status".to_owned(),
        "--xml".to_owned(),
        "-u".to_owned(),
        "-q".to_owned(),
        "--".to_owned(),
        ".".to_owned(),
    ];
    let output = match service
        .run(
            RunRequest::new(&text(params, "workspacePath"), args)
                .credentials(credentials(params))
                .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let root = super::xml::parse(&output.stdout);
    let mut out_of_date_count = 0u64;
    let mut head_revision = 0i64;
    for target in children(root.as_ref(), "target") {
        head_revision =
            head_revision.max(int_attr(child(Some(target), "against"), "revision").unwrap_or(0));
        for entry in children(Some(target), "entry") {
            let repos_item = child(Some(entry), "repos-status")
                .and_then(|node| node.attrs.get("item"))
                .map(String::as_str);
            if repos_item.is_some_and(|item| item != "none") {
                out_of_date_count += 1;
            }
        }
    }
    json!({
        "ok": true,
        "headRevision": head_revision,
        "outOfDateCount": out_of_date_count,
    })
}

pub(crate) async fn list_branches(service: &SvnService, params: &Value) -> Value {
    let workspace = match read_workspace_info(service, params).await {
        Ok(workspace) => workspace,
        Err(error) => return error,
    };
    let Some(root_url) = workspace.info.repository_root.clone() else {
        return failure("not-working-copy", "repository root is unavailable");
    };
    let mut result = json!({
        "ok": true,
        "layout": "non-standard",
        "branches": Vec::<Value>::new(),
    });
    if let Some(branch) = &workspace.branch {
        result["current"] = branch.to_json();
    }
    let root_args = vec![
        "list".to_owned(),
        "--xml".to_owned(),
        "--".to_owned(),
        format!("{root_url}@HEAD"),
    ];
    let root_list = match service
        .run(
            RunRequest::new(&text(params, "workspacePath"), root_args)
                .credentials(credentials(params))
                .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let top_level: BTreeSet<String> = parse_list(&root_list.stdout)
        .into_iter()
        .filter(|entry| entry.kind == "dir")
        .map(|entry| entry.name)
        .collect();
    if !top_level.contains("trunk") || !top_level.contains("branches") {
        return result;
    }
    let branch_args = vec![
        "list".to_owned(),
        "--xml".to_owned(),
        "--".to_owned(),
        format!("{root_url}/branches@HEAD"),
    ];
    let branch_list = match service
        .run(
            RunRequest::new(&text(params, "workspacePath"), branch_args)
                .credentials(credentials(params))
                .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let mut branches = vec![derive_branch_ref("trunk")];
    let mut named: Vec<super::BranchRef> = parse_list(&branch_list.stdout)
        .into_iter()
        .filter(|entry| entry.kind == "dir" && !entry.name.is_empty())
        .map(|entry| derive_branch_ref(&format!("branches/{}", entry.name)))
        .collect();
    named.sort_by(|left, right| left.name.cmp(&right.name));
    branches.extend(named);
    result["layout"] = Value::String("standard".to_owned());
    result["branches"] = Value::Array(branches.iter().map(super::BranchRef::to_json).collect());
    result
}
