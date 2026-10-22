//! 分支拓扑只读视图（TS `svnGraph.ts`；spec：docs/svn/architecture.md「分支图谱」）。
//!
//! copy 边来自每个分支的 `log --stop-on-copy`；merge 边来自仓库内 `svn:mergeinfo`。

use std::collections::{BTreeMap, HashSet};

use futures_util::stream::{self, StreamExt};
use serde_json::{Value, json};

use super::config::{
    GRAPH_COPY_CONCURRENCY, GRAPH_COPY_LOG_LIMIT, GRAPH_MAX_NODES, REMOTE_READ_TIMEOUT,
};
use super::errors::failure;
use super::parse::{parse_list, parse_log, parse_mergeinfo};
use super::paths::percent_decode;
use super::reads::read_workspace_info;
use super::{RunRequest, SvnService, credentials, text};

struct Candidate {
    relative_path: String,
    kind: &'static str,
    name: String,
}

fn node_name(relative_path: &str) -> String {
    relative_path
        .rsplit('/')
        .next()
        .unwrap_or(relative_path)
        .to_owned()
}

fn encode_repository_path(relative_path: &str) -> String {
    relative_path
        .split('/')
        .map(super::paths::encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn repository_relative_path(path: &str, root_url: &str) -> String {
    let root = root_url.trim_end_matches('/');
    let mut value = path;
    if value == root || value.starts_with(&format!("{root}/")) {
        value = &value[root.len()..];
    }
    percent_decode(value).trim_matches('/').to_owned()
}

/// `/trunk/src` → `trunk`，`/branches/x/src` → `branches/x`；只返回已知节点。
fn branch_from_repository_path(path: &str, known: &HashSet<String>) -> Option<String> {
    let normalized = path.trim_matches('/');
    if normalized == "trunk" || normalized.starts_with("trunk/") {
        return known.contains("trunk").then(|| "trunk".to_owned());
    }
    for prefix in ["branches/", "tags/"] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            let name = rest.split('/').next().unwrap_or_default();
            let candidate = format!("{prefix}{name}");
            return (!name.is_empty() && known.contains(&candidate)).then_some(candidate);
        }
    }
    None
}

async fn list_directories(
    service: &SvnService,
    params: &Value,
    url: &str,
) -> Result<Vec<String>, Value> {
    let output = service
        .run(
            RunRequest::new(
                &text(params, "workspacePath"),
                vec![
                    "list".to_owned(),
                    "--xml".to_owned(),
                    "--".to_owned(),
                    format!("{url}@HEAD"),
                ],
            )
            .credentials(credentials(params))
            .timeout(REMOTE_READ_TIMEOUT),
        )
        .await?;
    Ok(parse_list(&output.stdout)
        .into_iter()
        .filter(|entry| entry.kind == "dir")
        .map(|entry| entry.name)
        .collect())
}

async fn read_copy_origin(
    service: &SvnService,
    params: &Value,
    root_url: &str,
    relative_path: &str,
    known: &HashSet<String>,
) -> Option<(i64, Option<String>)> {
    let url = format!("{root_url}/{}", encode_repository_path(relative_path));
    let output = service
        .run(
            RunRequest::new(
                &text(params, "workspacePath"),
                vec![
                    "log".to_owned(),
                    "--xml".to_owned(),
                    "-v".to_owned(),
                    "--limit".to_owned(),
                    GRAPH_COPY_LOG_LIMIT.to_string(),
                    "--stop-on-copy".to_owned(),
                    "--".to_owned(),
                    format!("{url}@HEAD"),
                ],
            )
            .credentials(credentials(params))
            .timeout(REMOTE_READ_TIMEOUT),
        )
        .await
        .ok()?;
    let entries = parse_log(&output.stdout);
    let oldest = entries.last()?;
    let target = format!("/{relative_path}");
    let created = oldest
        .changed_paths
        .iter()
        .find(|changed| changed.action == "A" && changed.path.trim_end_matches('/') == target)?;
    let from = created
        .copy_from_path
        .as_deref()
        .and_then(|path| branch_from_repository_path(path, known));
    Some((oldest.revision, from))
}

fn collect_merge_edges(xml: &str, known: &HashSet<String>, root_url: &str) -> Vec<Value> {
    let mut edges: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for entry in parse_mergeinfo(xml) {
        let Some(to) =
            branch_from_repository_path(&repository_relative_path(&entry.path, root_url), known)
        else {
            continue;
        };
        for line in entry.mergeinfo.lines() {
            let trimmed = line.trim();
            let Some(split_at) = trimmed.rfind(':') else {
                continue;
            };
            if split_at == 0 {
                continue;
            }
            let Some(from) = branch_from_repository_path(&trimmed[..split_at], known) else {
                continue;
            };
            let revisions = &trimmed[split_at + 1..];
            if from == to || revisions.is_empty() {
                continue;
            }
            let entry = edges.entry((from, to.clone())).or_default();
            for part in revisions.split(',') {
                let part = part.trim();
                if !part.is_empty() && !entry.iter().any(|value| value == part) {
                    entry.push(part.to_owned());
                }
            }
        }
    }
    edges
        .into_iter()
        .map(|((from, to), revisions)| {
            json!({
                "kind": "merge",
                "from": from,
                "to": to,
                "revisions": revisions.join(","),
            })
        })
        .collect()
}

pub(crate) async fn get_branch_graph(service: &SvnService, params: &Value) -> Value {
    let workspace_path = text(params, "workspacePath");
    let workspace = match read_workspace_info(service, params).await {
        Ok(workspace) => workspace,
        Err(error) => return error,
    };
    let Some(root_url) = workspace.info.repository_root.clone() else {
        return failure("not-working-copy", "repository root is unavailable");
    };
    let top_level = match list_directories(service, params, &root_url).await {
        Ok(names) => names,
        Err(error) => return error,
    };
    let non_standard = !top_level.iter().any(|name| name == "trunk")
        || !top_level.iter().any(|name| name == "branches");
    if non_standard {
        let nodes = workspace
            .branch
            .as_ref()
            .map(|branch| {
                vec![json!({
                    "relativePath": branch.relative_path,
                    "kind": branch.kind,
                    "name": branch.name,
                })]
            })
            .unwrap_or_default();
        return json!({
            "ok": true,
            "layout": "non-standard",
            "nodes": nodes,
            "edges": Vec::<Value>::new(),
            "truncated": false,
        });
    }

    let mut candidates = vec![Candidate {
        relative_path: "trunk".to_owned(),
        kind: "trunk",
        name: "trunk".to_owned(),
    }];
    for (directory, kind) in [("branches", "branch"), ("tags", "tag")] {
        if !top_level.iter().any(|name| name == directory) {
            continue;
        }
        let names =
            match list_directories(service, params, &format!("{root_url}/{directory}")).await {
                Ok(names) => names,
                Err(error) => return error,
            };
        let mut names = names;
        names.sort();
        for name in names {
            candidates.push(Candidate {
                relative_path: format!("{directory}/{name}"),
                kind,
                name: node_name(&name),
            });
        }
    }
    let truncated = candidates.len() > GRAPH_MAX_NODES;
    candidates.truncate(GRAPH_MAX_NODES);
    let known: HashSet<String> = candidates
        .iter()
        .map(|candidate| candidate.relative_path.clone())
        .collect();

    let mut origin_futures = Vec::with_capacity(candidates.len());
    let root_url_ref = &root_url;
    let known_ref = &known;
    for candidate in &candidates {
        let relative_path = candidate.relative_path.clone();
        let is_trunk = candidate.kind == "trunk";
        origin_futures.push(async move {
            if is_trunk {
                None
            } else {
                read_copy_origin(service, params, root_url_ref, &relative_path, known_ref).await
            }
        });
    }
    let origins: Vec<Option<(i64, Option<String>)>> = stream::iter(origin_futures)
        .buffered(GRAPH_COPY_CONCURRENCY)
        .collect()
        .await;

    let mut nodes: Vec<Value> = Vec::with_capacity(candidates.len());
    let mut copy_edges: Vec<Value> = Vec::new();
    for (candidate, origin) in candidates.iter().zip(origins) {
        let mut node = json!({
            "relativePath": candidate.relative_path,
            "kind": candidate.kind,
            "name": candidate.name,
        });
        if let Some((revision, from)) = origin {
            node["createdRevision"] = Value::from(revision);
            if let Some(from) = from {
                copy_edges.push(json!({
                    "kind": "copy",
                    "from": from,
                    "to": candidate.relative_path,
                    "revision": revision,
                }));
                node["createdFrom"] = Value::String(from);
            }
        }
        nodes.push(node);
    }
    copy_edges.sort_by(|left, right| {
        left["to"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["to"].as_str().unwrap_or_default())
    });

    let mergeinfo = match service
        .run(
            RunRequest::new(
                &workspace_path,
                vec![
                    "propget".to_owned(),
                    "--xml".to_owned(),
                    "-R".to_owned(),
                    "svn:mergeinfo".to_owned(),
                    "--".to_owned(),
                    format!("{root_url}@HEAD"),
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
    let mut edges = copy_edges;
    edges.extend(collect_merge_edges(&mergeinfo.stdout, &known, &root_url));

    json!({
        "ok": true,
        "layout": "standard",
        "nodes": nodes,
        "edges": edges,
        "truncated": truncated,
    })
}
