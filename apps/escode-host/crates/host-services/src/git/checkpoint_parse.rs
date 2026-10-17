//! `git-checkpoint` 的输出解析（TS `gitCheckpointHelpers.ts` 的 `parseNameStatus` / `parseNumstat` / `parseLsTree`）。
//! 三个都吃 `-z` 的 NUL 分隔输出，逐字复刻 TS 的记录推进方式（含畸形记录跳过）。

use super::paths::normalize_git_path;

/// TS `GitCheckpointNameStatusEntry`。
pub struct NameStatusEntry {
    pub kind: &'static str,
    pub path: String,
    pub original_path: Option<String>,
}

/// TS `mapNameStatusKind`：取状态首个字符，`null`/缺省回 `modified`。
fn map_kind(status: &str) -> &'static str {
    match status.chars().next() {
        Some('A') => "added",
        Some('D') => "deleted",
        Some('R') | Some('C') => "renamed",
        _ => "modified",
    }
}

/// `git diff --name-status -z`：非 rename 是 `[status, path]`，rename/copy 是 `[status, old, new]`。
/// TS 用下标越界即 `null` 判缺，Rust 用 `get` 同义；缺任一端的记录整条跳过（不补位）。
pub fn parse_name_status(stdout: &str) -> Vec<NameStatusEntry> {
    let records: Vec<&str> = stdout.split('\0').filter(|r| !r.is_empty()).collect();
    let mut entries = Vec::new();
    let mut index = 0usize;
    while index < records.len() {
        let kind = map_kind(records[index]);
        if kind == "renamed" {
            let original = records.get(index + 1).copied().unwrap_or_default();
            let path = records.get(index + 2).copied().unwrap_or_default();
            index += 3;
            // TS 的 `!originalPath || !path`：空串与缺项同判。
            if original.is_empty() || path.is_empty() {
                continue;
            }
            entries.push(NameStatusEntry {
                kind,
                path: normalize_git_path(path),
                original_path: Some(normalize_git_path(original)),
            });
            continue;
        }
        let path = records.get(index + 1).copied().unwrap_or_default();
        index += 2;
        if path.is_empty() {
            continue;
        }
        entries.push(NameStatusEntry {
            kind,
            path: normalize_git_path(path),
            original_path: None,
        });
    }
    entries
}

/// TS `parseNumstat`：`added<NUL>removed<NUL>path`；rename 的 path 字段为空，改从后续两条记录取。
/// 返回 `path → (added, removed)`；rename 会同时写**新路径与旧路径**两条（TS 同）。
pub fn parse_numstat(stdout: &str) -> Vec<(String, u64, u64)> {
    let records: Vec<&str> = stdout.split('\0').filter(|r| !r.is_empty()).collect();
    let mut stats = Vec::new();
    let mut index = 0usize;
    while index < records.len() {
        let fields: Vec<&str> = records[index].split('\t').collect();
        if fields.len() < 3 {
            index += 1;
            continue;
        }
        let count = |field: &str| -> u64 {
            // TS `parseInt(v, 10) || 0`：`-`（二进制）与非法值都归 0。
            if field == "-" {
                return 0;
            }
            field.trim().parse::<u64>().unwrap_or(0)
        };
        let added = count(fields[0]);
        let removed = count(fields[1]);
        // TS `fields.slice(2).join("\t")`：路径本身含 tab 时由此还原。
        let path_field = fields[2..].join("\t");
        if !path_field.is_empty() {
            stats.push((normalize_git_path(&path_field), added, removed));
            index += 1;
            continue;
        }
        let original = records.get(index + 1).copied().unwrap_or_default();
        let renamed = records.get(index + 2).copied().unwrap_or_default();
        index += 3;
        if renamed.is_empty() {
            continue;
        }
        stats.push((normalize_git_path(renamed), added, removed));
        if !original.is_empty() {
            stats.push((normalize_git_path(original), added, removed));
        }
    }
    stats
}

/// TS `GitTreeEntry`。
pub struct TreeEntry {
    pub mode: String,
    pub object_id: String,
}

/// `git ls-tree -r -z`：`<mode> <type> <objectId>\t<path>`。
pub fn parse_ls_tree(stdout: &str) -> Vec<(String, TreeEntry)> {
    let mut entries = Vec::new();
    for record in stdout.split('\0').filter(|r| !r.is_empty()) {
        let Some(tab) = record.find('\t') else {
            continue;
        };
        let header: Vec<&str> = record[..tab].split(' ').collect();
        if header.len() < 3 {
            continue;
        }
        let path = normalize_git_path(&record[tab + 1..]);
        entries.push((
            path,
            TreeEntry {
                mode: header[0].to_owned(),
                object_id: header[2].to_owned(),
            },
        ));
    }
    entries
}

#[cfg(test)]
#[path = "checkpoint_parse_tests.rs"]
mod tests;
