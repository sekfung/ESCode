//! svn 提交信息生成的输入收集（文件摘要 + 最多 8 个 diff），与 Git 共用同一生成器。

use serde_json::{Value, json};

use super::config::normalize_svn_path;
use super::runtime::SvnService;
use super::{reads, strings, text};

impl SvnService {
    pub async fn commit_message_inputs(
        &self,
        params: &Value,
    ) -> anyhow::Result<(Option<String>, Vec<Value>, Vec<Value>)> {
        let selected: std::collections::BTreeSet<String> = strings(params, "paths")
            .into_iter()
            .map(|path| normalize_svn_path(&path))
            .collect();
        let changes = reads::read_changes(self, params).await;
        if changes["ok"].as_bool() != Some(true) {
            anyhow::bail!(
                "{}",
                changes["message"]
                    .as_str()
                    .unwrap_or("failed to read svn changes")
            );
        }
        let change_items: Vec<&Value> = changes["changes"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|change| selected.contains(change["path"].as_str().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();
        let files: Vec<Value> = change_items
            .iter()
            .map(|change| {
                let status = change["status"].as_str().unwrap_or("modified");
                let kind = match status {
                    "added" | "unversioned" => "added",
                    "deleted" | "missing" => "deleted",
                    _ => "modified",
                };
                json!({
                    "kind": kind,
                    "repoRelativePath": change["path"],
                    "added": change["additions"],
                    "removed": change["deletions"],
                })
            })
            .collect();
        if files.is_empty() {
            anyhow::bail!("There are no changes available to commit.");
        }
        let mut diffs = Vec::new();
        for change in change_items.iter().take(8) {
            if change["isDirectory"].as_bool() == Some(true)
                || change["isBinary"].as_bool() == Some(true)
            {
                continue;
            }
            let path = change["path"].as_str().unwrap_or_default();
            let diff = reads::read_diff(
                self,
                &json!({
                    "workspacePath": text(params, "workspacePath"),
                    "workspaceIdentity": params.get("workspaceIdentity").cloned(),
                    "path": path,
                }),
            )
            .await;
            if diff["ok"].as_bool() == Some(true)
                && let Some(patch) = diff["patch"].as_str()
            {
                diffs.push(json!({
                    "path": path,
                    "patch": patch,
                    "summary": Value::Null,
                }));
            }
        }
        let branch_name = match reads::read_workspace_info(self, params).await {
            Ok(workspace) => workspace.branch.map(|branch| branch.name),
            Err(_) => None,
        };
        Ok((branch_name, files, diffs))
    }
}
