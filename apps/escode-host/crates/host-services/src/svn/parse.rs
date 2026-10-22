//! svn XML → 业务类型（TS `repo/svnParse.ts`），以及不依赖本地化文本的计数/归类。

use serde_json::{Value, json};

use super::config::normalize_svn_path;
use super::xml::{XmlNode, child, children, int_attr, parse, path, text};

#[derive(Clone, Debug, Default)]
pub struct TreeConflict {
    pub operation: Option<String>,
    pub action: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct InfoConflict {
    pub kinds: Vec<&'static str>,
    pub artifact_paths: Vec<String>,
    pub tree_conflict: Option<TreeConflict>,
}

#[derive(Clone, Debug, Default)]
pub struct InfoEntry {
    pub path: String,
    pub revision: Option<i64>,
    pub url: Option<String>,
    pub repository_root: Option<String>,
    pub repository_uuid: Option<String>,
    pub wc_root: Option<String>,
    pub conflict: Option<InfoConflict>,
}

#[derive(Clone, Debug, Default)]
pub struct StatusEntry {
    pub path: String,
    pub item: String,
    pub props: String,
    pub tree_conflicted: bool,
    /// externals 定义下的条目属于其他工作副本，第一期只读展示。
    pub in_external: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LogPath {
    pub action: String,
    pub path: String,
    pub kind: Option<String>,
    pub copy_from_path: Option<String>,
    pub copy_from_revision: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct LogEntry {
    pub revision: i64,
    pub author: String,
    pub date: String,
    pub message: String,
    pub changed_paths: Vec<LogPath>,
}

#[derive(Clone, Debug, Default)]
pub struct ListEntry {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Default)]
pub struct BlameLine {
    pub line_number: i64,
    pub revision: i64,
    pub author: String,
    pub content: String,
}

#[derive(Clone, Debug, Default)]
pub struct MergeinfoEntry {
    pub path: String,
    pub mergeinfo: String,
}

/// 解析 `svn propget --xml -R svn:mergeinfo` 的 properties/target/property 结构。
pub fn parse_mergeinfo(xml: &str) -> Vec<MergeinfoEntry> {
    let root = parse(xml);
    let mut entries = Vec::new();
    for target in children(root.as_ref(), "target") {
        let Some(path) = target.attrs.get("path").cloned() else {
            continue;
        };
        for property in children(Some(target), "property") {
            if property
                .attrs
                .get("name")
                .is_some_and(|name| name == "svn:mergeinfo")
            {
                entries.push(MergeinfoEntry {
                    path: path.clone(),
                    mergeinfo: property.text.clone(),
                });
            }
        }
    }
    entries
}

/// 解析 `svn blame --xml`；超过 max_lines 时只保留前 max_lines 行。
pub fn parse_blame(xml: &str, max_lines: usize) -> (Vec<BlameLine>, bool) {
    let root = parse(xml);
    let mut lines: Vec<BlameLine> = Vec::new();
    let mut truncated = false;
    'outer: for target in children(root.as_ref(), "target") {
        for entry in children(Some(target), "entry") {
            if lines.len() >= max_lines {
                truncated = true;
                break 'outer;
            }
            let commit = child(Some(entry), "commit");
            lines.push(BlameLine {
                line_number: int_attr(Some(entry), "line-number")
                    .unwrap_or((lines.len() + 1) as i64),
                revision: int_attr(commit, "revision").unwrap_or(0),
                author: trimmed_text(child(commit, "author")).unwrap_or_default(),
                content: text(child(Some(entry), "token")).unwrap_or_default(),
            });
        }
    }
    (lines, truncated)
}

fn trimmed_text(node: Option<&XmlNode>) -> Option<String> {
    text(node).map(|value| value.trim().to_owned())
}

pub fn parse_info(xml: &str) -> Vec<InfoEntry> {
    let root = parse(xml);
    children(root.as_ref(), "entry")
        .into_iter()
        .map(|entry| {
            let wc_info = child(Some(entry), "wc-info");
            let conflicts = children(Some(entry), "conflict");
            let tree_conflict = child(Some(entry), "tree-conflict");
            let mut kinds: Vec<&'static str> = Vec::new();
            let mut artifact_paths: Vec<String> = Vec::new();
            for conflict in conflicts {
                let kind = if conflict
                    .attrs
                    .get("type")
                    .is_some_and(|value| value == "property")
                {
                    "property"
                } else {
                    "text"
                };
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
                for name in [
                    "prev-base-file",
                    "prev-wc-file",
                    "cur-base-file",
                    "prop-file",
                ] {
                    if let Some(file) = trimmed_text(child(Some(conflict), name))
                        && !file.is_empty()
                    {
                        artifact_paths.push(file);
                    }
                }
            }
            if tree_conflict.is_some() && !kinds.contains(&"tree") {
                kinds.push("tree");
            }
            InfoEntry {
                path: entry
                    .attrs
                    .get("path")
                    .cloned()
                    .unwrap_or_else(|| ".".to_owned()),
                revision: int_attr(Some(entry), "revision"),
                url: trimmed_text(child(Some(entry), "url")),
                repository_root: trimmed_text(path(Some(entry), &["repository", "root"])),
                repository_uuid: trimmed_text(path(Some(entry), &["repository", "uuid"])),
                wc_root: trimmed_text(child(wc_info, "wcroot-abspath")),
                conflict: (!kinds.is_empty()).then(|| InfoConflict {
                    kinds,
                    artifact_paths,
                    tree_conflict: tree_conflict.map(|node| TreeConflict {
                        operation: node.attrs.get("operation").cloned(),
                        action: node.attrs.get("action").cloned(),
                        reason: node.attrs.get("reason").cloned(),
                    }),
                }),
            }
        })
        .collect()
}

pub fn parse_status(xml: &str) -> Vec<StatusEntry> {
    let root = parse(xml);
    let mut entries: Vec<StatusEntry> = Vec::new();
    for target in children(root.as_ref(), "target") {
        for entry in children(Some(target), "entry") {
            let status = child(Some(entry), "wc-status");
            let attr = |name: &str| {
                status
                    .and_then(|node| node.attrs.get(name))
                    .cloned()
                    .unwrap_or_else(|| default_status_attr(name).to_owned())
            };
            entries.push(StatusEntry {
                path: normalize_svn_path(entry.attrs.get("path").map_or(".", String::as_str)),
                item: attr("item"),
                props: attr("props"),
                tree_conflicted: status
                    .and_then(|node| node.attrs.get("tree-conflicted"))
                    .is_some_and(|value| value == "true"),
                in_external: false,
            });
        }
    }
    // 状态输出会把每个 external 作为单独的 target 追加，路径以外部目录为前缀。
    let external_roots: Vec<String> = entries
        .iter()
        .filter(|entry| entry.item == "external")
        .map(|entry| entry.path.clone())
        .collect();
    for entry in &mut entries {
        entry.in_external = external_roots
            .iter()
            .any(|root| entry.path != *root && entry.path.starts_with(&format!("{root}/")));
    }
    entries
}

fn default_status_attr(name: &str) -> &'static str {
    match name {
        "item" => "normal",
        _ => "none",
    }
}

/// TS `toSvnItemStatus`；`None` 表示不展示（normal / ignored 等）。
pub fn item_status(entry: &StatusEntry) -> Option<&'static str> {
    if entry.tree_conflicted || entry.item == "conflicted" || entry.props == "conflicted" {
        return Some("conflicted");
    }
    match entry.item.as_str() {
        "modified" | "merged" => Some("modified"),
        "added" => Some("added"),
        "deleted" => Some("deleted"),
        "replaced" => Some("replaced"),
        "unversioned" => Some("unversioned"),
        "missing" => Some("missing"),
        "obstructed" | "incomplete" => Some("obstructed"),
        "external" => Some("external"),
        _ if entry.props == "modified" => Some("props-modified"),
        _ => None,
    }
}

pub fn parse_log(xml: &str) -> Vec<LogEntry> {
    let root = parse(xml);
    children(root.as_ref(), "logentry")
        .into_iter()
        .map(|entry| LogEntry {
            revision: int_attr(Some(entry), "revision").unwrap_or(0),
            author: trimmed_text(child(Some(entry), "author")).unwrap_or_default(),
            date: trimmed_text(child(Some(entry), "date")).unwrap_or_default(),
            message: text(child(Some(entry), "msg")).unwrap_or_default(),
            changed_paths: children(child(Some(entry), "paths"), "path")
                .into_iter()
                .map(|node| {
                    let action = node.attrs.get("action").map_or("M", String::as_str);
                    LogPath {
                        action: if ["A", "M", "D", "R"].contains(&action) {
                            action.to_owned()
                        } else {
                            "M".to_owned()
                        },
                        path: node.text.trim().to_owned(),
                        kind: node
                            .attrs
                            .get("kind")
                            .filter(|kind| *kind == "file" || *kind == "dir")
                            .cloned(),
                        copy_from_path: node.attrs.get("copyfrom-path").cloned(),
                        copy_from_revision: int_attr(Some(node), "copyfrom-rev"),
                    }
                })
                .collect(),
        })
        .collect()
}

pub fn parse_list(xml: &str) -> Vec<ListEntry> {
    let root = parse(xml);
    children(root.as_ref(), "list")
        .into_iter()
        .flat_map(|list| children(Some(list), "entry"))
        .map(|entry| ListEntry {
            name: trimmed_text(child(Some(entry), "name")).unwrap_or_default(),
            kind: if entry.attrs.get("kind").is_some_and(|value| value == "file") {
                "file".to_owned()
            } else {
                "dir".to_owned()
            },
        })
        .collect()
}

/// 统计 update / switch 输出中的变更行；前四列是固定状态字母，不受本地化影响。
pub fn count_updated_paths(stdout: &str) -> usize {
    stdout
        .lines()
        .filter(|line| {
            let bytes = line.as_bytes();
            if bytes.len() < 6 {
                return false;
            }
            let status = |index: usize, set: &[u8]| set.contains(&bytes[index]);
            status(0, b"ADUCGER ")
                && status(1, b"ADUCGE ")
                && status(2, b"B ")
                && status(3, b"C ")
                && bytes[..4].iter().any(u8::is_ascii_graphic)
                && bytes[4].is_ascii_whitespace()
                && bytes.iter().skip(4).any(|byte| !byte.is_ascii_whitespace())
        })
        .count()
}

/// 提取 commit 输出最后一个数字作为新修订号（`Committed revision 5.` → 5）。
pub fn parse_committed_revision(stdout: &str) -> Option<i64> {
    let last_line = stdout.lines().rev().find(|line| !line.trim().is_empty())?;
    let without_trailing = last_line.trim_end_matches(|ch: char| !ch.is_ascii_digit());
    let digits = without_trailing
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    digits.parse().ok()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DiffCount {
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
}

/// 统计 diff 增删行；只计 hunk，属性段与文件头不计。
pub fn count_diff_lines(patch: &str) -> DiffCount {
    let mut count = DiffCount::default();
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("Index: ") || line.starts_with("Property changes on: ") {
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@ ") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            if line.starts_with("svn:mime-type = ") {
                count.is_binary = true;
            }
            continue;
        }
        if line.starts_with('+') {
            count.additions += 1;
        } else if line.starts_with('-') {
            count.deletions += 1;
        }
    }
    count
}

impl LogEntry {
    pub fn to_json(&self) -> Value {
        json!({
            "revision": self.revision,
            "author": self.author,
            "date": self.date,
            "message": self.message,
            "changedPaths": self.changed_paths.iter().map(|path| {
                let mut value = json!({"action": path.action, "path": path.path});
                if let Some(kind) = &path.kind {
                    value["kind"] = Value::String(kind.clone());
                }
                if let Some(copy_from_path) = &path.copy_from_path {
                    value["copyFromPath"] = Value::String(copy_from_path.clone());
                }
                if let Some(revision) = path.copy_from_revision {
                    value["copyFromRevision"] = Value::from(revision);
                }
                value
            }).collect::<Vec<_>>(),
        })
    }
}
