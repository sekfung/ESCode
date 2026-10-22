//! `svn merge` 写操作（TS `svnWrites.merge`；spec：docs/svn/architecture.md）。

use escode_host_control::logging;
use serde_json::{Value, json};

use super::config::LONG_WRITE_TIMEOUT;
use super::errors::failure;
use super::parse::count_updated_paths;
use super::paths::encode_segment;
use super::reads::{count_conflicts, read_workspace_info};
use super::{RunRequest, SvnService, credentials, text, workspace_key};

pub(crate) async fn merge(service: &SvnService, params: &Value) -> Value {
    let raw = text(params, "relativeSourcePath");
    let relative_source = raw
        .strip_prefix('^')
        .unwrap_or(&raw)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned();
    if relative_source.is_empty()
        || relative_source.split('/').any(|segment| segment == "..")
        || relative_source.contains(['\r', '\n'])
    {
        return failure("invalid-request", "invalid branch path");
    }
    let has_start = params.get("startRevision").is_some();
    let has_end = params.get("endRevision").is_some();
    if has_start != has_end {
        return failure(
            "invalid-request",
            "revision range requires both start and end",
        );
    }
    let start = params.get("startRevision").and_then(Value::as_i64);
    let end = params.get("endRevision").and_then(Value::as_i64);
    if has_start && has_end {
        let valid = start.is_some_and(|value| value >= 1)
            && end.is_some_and(|value| value >= 1)
            && start.zip(end).is_some_and(|(start, end)| start <= end);
        if !valid {
            return failure("invalid-request", "invalid revision range");
        }
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
            let encoded = relative_source
                .split('/')
                .map(encode_segment)
                .collect::<Vec<_>>()
                .join("/");
            let target_url = format!("{repository_root}/{encoded}");
            // UI 的起止修订是闭区间；svn 的 -r 表示两个修订之间的差异，
            // 因此区间换成 start-1:end；单个修订用 -c（也避免 r0）。
            let range: Vec<String> = match (start, end) {
                (Some(start), Some(end)) if start == end => {
                    vec!["-c".to_owned(), start.to_string()]
                }
                (Some(start), Some(end)) => {
                    vec!["-r".to_owned(), format!("{}:{end}", start.saturating_sub(1).max(1))]
                }
                _ => Vec::new(),
            };
            let mut args = vec![
                "merge".to_owned(),
                "--accept".to_owned(),
                "postpone".to_owned(),
            ];
            args.extend(range.iter().cloned());
            args.extend([
                "--".to_owned(),
                format!("{target_url}@HEAD"),
                ".".to_owned(),
            ]);
            let output = match service
                .run(
                    RunRequest::new(&wc_root, args)
                        .credentials(credentials(params))
                        .timeout(LONG_WRITE_TIMEOUT),
                )
                .await
            {
                Ok(output) => output,
                Err(error) => return error,
            };
            let conflict_count = count_conflicts(service, &text(params, "workspacePath")).await;
            let merged_file_count = count_updated_paths(&output.stdout);
            logging::info(
                "svn",
                format!(
                    "svn 合并完成 workspaceKey={key} source={relative_source} mergedFileCount={merged_file_count} conflictCount={conflict_count}"
                ),
            );
            json!({
                "ok": true,
                "mergedFileCount": merged_file_count,
                "conflictCount": conflict_count,
            })
        })
        .await
}
