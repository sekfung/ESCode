//! 知识图谱投影（spec §11）：条目与来源为节点，related / supersedes / source / conflict 为边。
//! 纯投影：输入是已按作用域过滤的条目、来源与冲突；输出直接作为 `v4/wiki/graph` 结果。
use super::conflicts::Conflict;
use super::entry::{Entry, Kind, Scope, Status};
use super::source::SourceRecord;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_NODES: usize = 2_000;

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub scopes: Option<Vec<Scope>>,
    pub statuses: Option<Vec<Status>>,
    pub kinds: Option<Vec<Kind>>,
    pub include_sources: bool,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: &'static str,
    pub label: String,
    pub scope: Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validity: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_status: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub edge_type: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub truncated: bool,
    pub counts: BTreeMap<&'static str, usize>,
}

pub fn entry_node_id(scope: Scope, id: &str) -> String {
    format!("entry:{}:{id}", scope.as_str())
}

fn source_node_id(scope: Scope, document_id: &str) -> String {
    format!("source:{}:{document_id}", scope.as_str())
}

fn keep(entry: &Entry, filter: &Filter) -> bool {
    filter
        .scopes
        .as_ref()
        .is_none_or(|s| s.contains(&entry.meta.scope))
        && filter
            .statuses
            .as_ref()
            .is_none_or(|s| s.contains(&entry.meta.status))
        && filter
            .kinds
            .as_ref()
            .is_none_or(|k| k.contains(&entry.meta.kind))
}

pub fn build(
    entries: &[Entry],
    sources: &[SourceRecord],
    conflicts: &[Conflict],
    filter: &Filter,
) -> Graph {
    let limit = filter.limit.clamp(1, MAX_NODES);
    let mut kept: Vec<&Entry> = entries.iter().filter(|e| keep(e, filter)).collect();
    // 截断时保留更「重要」的条目：已批准优先，其次按更新时间倒序。
    kept.sort_by(|a, b| {
        (a.meta.status != Status::Approved)
            .cmp(&(b.meta.status != Status::Approved))
            .then(b.meta.updated_at.cmp(&a.meta.updated_at))
            .then(a.meta.id.cmp(&b.meta.id))
    });
    let truncated = kept.len() > limit;
    kept.truncate(limit);
    let present: BTreeSet<(Scope, &str)> = kept
        .iter()
        .map(|e| (e.meta.scope, e.meta.id.as_str()))
        .collect();
    let resolve = |id: &str| -> Option<String> {
        [Scope::Project, Scope::Global]
            .into_iter()
            .find(|s| present.contains(&(*s, id)))
            .map(|s| entry_node_id(s, id))
    };
    let mut nodes = vec![];
    let mut edges = vec![];
    let mut edge_ids = BTreeSet::new();
    let mut push_edge =
        |from: String, to: String, edge_type: &'static str, edges: &mut Vec<Edge>| {
            let (a, b) = if edge_type == "related" || edge_type == "conflict" {
                if from <= to { (from, to) } else { (to, from) }
            } else {
                (from, to)
            };
            let id = format!("{edge_type}:{a}->{b}");
            if a != b && edge_ids.insert(id.clone()) {
                edges.push(Edge {
                    id,
                    from: a,
                    to: b,
                    edge_type,
                });
            }
        };
    for entry in &kept {
        let node_id = entry_node_id(entry.meta.scope, &entry.meta.id);
        nodes.push(Node {
            id: node_id.clone(),
            node_type: "entry",
            label: entry.meta.title.clone(),
            scope: entry.meta.scope,
            entry_id: Some(entry.meta.id.clone()),
            kind: Some(entry.meta.kind.as_str()),
            status: Some(entry.meta.status.as_str()),
            authority: Some(entry.meta.authority.as_str()),
            validity: Some(entry.meta.validity.state.as_str()),
            format: None,
            source_status: None,
        });
        for target in &entry.meta.related {
            if let Some(to) = resolve(target) {
                push_edge(node_id.clone(), to, "related", &mut edges);
            }
        }
        if let Some(to) = entry.meta.supersedes.as_deref().and_then(resolve) {
            push_edge(node_id.clone(), to, "supersedes", &mut edges);
        }
        if filter.include_sources {
            for source in &entry.meta.sources {
                push_edge(
                    node_id.clone(),
                    source_node_id(entry.meta.scope, &source.document_id),
                    "source",
                    &mut edges,
                );
            }
        }
    }
    let mut source_count = 0;
    if filter.include_sources {
        let used: BTreeSet<&str> = edges
            .iter()
            .filter(|e| e.edge_type == "source")
            .map(|e| e.to.as_str())
            .collect();
        for source in sources {
            let id = source_node_id(source.scope, &source.document_id);
            if !used.contains(id.as_str()) {
                continue;
            }
            source_count += 1;
            nodes.push(Node {
                id,
                node_type: "source",
                label: source.source_path.clone(),
                scope: source.scope,
                entry_id: None,
                kind: None,
                status: None,
                authority: None,
                validity: None,
                format: Some(source.format.clone()),
                source_status: Some(source.status.as_str()),
            });
        }
        // 来源记录缺失（被手工删除）时去掉悬空边，图里不出现不存在的节点。
        let ids: BTreeSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
        edges.retain(|e| ids.contains(&e.from) && ids.contains(&e.to));
    }
    for conflict in conflicts {
        let parties: Vec<String> = conflict
            .parties
            .iter()
            .filter(|p| present.contains(&(p.scope, p.id.as_str())))
            .map(|p| entry_node_id(p.scope, &p.id))
            .collect();
        if let [a, b, ..] = parties.as_slice() {
            push_edge(a.clone(), b.clone(), "conflict", &mut edges);
        }
    }
    let mut counts = BTreeMap::new();
    counts.insert("entries", kept.len());
    counts.insert("sources", source_count);
    counts.insert("edges", edges.len());
    counts.insert("totalEntries", entries.len());
    Graph {
        nodes,
        edges,
        truncated,
        counts,
    }
}
