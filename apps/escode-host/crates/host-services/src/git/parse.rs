//! git 输出解析（TS `repo/gitCliHelpers.ts` 与 `gitCliRepo.ts` 的解析函数）。
//!
//! TS 正则在这里改写为等价的手写匹配（host-services 不引入 regex），边界语义逐条保持：
//! 例如 porcelain 记录的 `(.+)$` 中 JS `.` 不匹配换行，路径含换行的记录整条跳过。

use super::paths::{js_trim, normalize_git_path};

/// TS `GitStatusEntry`。
#[derive(Clone, Debug, PartialEq)]
pub struct StatusEntry {
    pub path: String,
    pub original_path: Option<String>,
    pub kind: &'static str,
    pub x: Option<String>,
    pub y: Option<String>,
    pub is_untracked: bool,
    pub is_conflicted: bool,
}

/// TS `parseStatusPorcelain` 的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedStatus {
    pub branch_name: Option<String>,
    pub tracking_branch_name: Option<String>,
    pub head_ref_type: &'static str,
    pub ahead: i64,
    pub behind: i64,
    pub entries: Vec<StatusEntry>,
}

/// TS `GitLineStat`。
#[derive(Clone, Debug, PartialEq)]
pub struct LineStat {
    pub added: i64,
    pub removed: i64,
    pub kind: Option<&'static str>,
    pub original_path: Option<String>,
}

/// 保持插入序的 numstat 表（TS `Map`：重复键覆盖值、位置不变）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NumstatMap(pub Vec<(String, LineStat)>);

impl NumstatMap {
    pub fn get(&self, path: &str) -> Option<&LineStat> {
        self.0
            .iter()
            .find(|(key, _)| key == path)
            .map(|(_, stat)| stat)
    }

    pub fn set(&mut self, path: String, stat: LineStat) {
        match self.0.iter_mut().find(|(key, _)| *key == path) {
            Some(slot) => slot.1 = stat,
            None => self.0.push((path, stat)),
        }
    }
}

/// JS `Number.parseInt(value, 10)`：前导空白、可选符号、十进制数字前缀；无数字 → None（NaN）。
pub fn js_parse_int(value: &str) -> Option<i64> {
    let trimmed = js_trim(value);
    let (sign, digits) = match trimmed.as_bytes().first() {
        Some(b'-') => (-1, &trimmed[1..]),
        Some(b'+') => (1, &trimmed[1..]),
        _ => (1, trimmed),
    };
    let end = digits
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    digits[..end]
        .parse::<i64>()
        .ok()
        .map(|number| sign * number)
}

/// JS 正则 `.` 不匹配的行终止符。
fn has_line_terminator(value: &str) -> bool {
    value.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

/// 按 `^<tag> ([^ ]{2})( [^ ]+){fields} (.+)$` 拆一条 porcelain 记录：返回 (XY, 路径)。
fn split_record(record: &str, fields: usize) -> Option<(String, String)> {
    let parts: Vec<&str> = record.splitn(fields + 3, ' ').collect();
    if parts.len() != fields + 3 {
        return None;
    }
    let xy = parts[1];
    if xy.chars().count() != 2 || parts[2..fields + 2].iter().any(|field| field.is_empty()) {
        return None;
    }
    let path = parts[fields + 2];
    if path.is_empty() || has_line_terminator(path) || has_line_terminator(xy) {
        return None;
    }
    Some((xy.to_owned(), path.to_owned()))
}

fn kind_from_status_code(code: char) -> &'static str {
    match code {
        'A' | '?' => "added",
        'D' => "deleted",
        'R' | 'C' => "renamed",
        _ => "modified",
    }
}

/// TS `parseBranchAheadBehind`：首个 `+N` 与首个 `-N`。
fn parse_ahead_behind(value: &str) -> (i64, i64) {
    let after = |marker: char| {
        value.match_indices(marker).find_map(|(index, _)| {
            let digits: String = value[index + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!digits.is_empty()).then(|| digits.parse::<i64>().unwrap_or(0))
        })
    };
    (after('+').unwrap_or(0), after('-').unwrap_or(0))
}

/// TS `parseStatusPorcelain`（`git status --porcelain=v2 --branch -z`）。
pub fn parse_status_porcelain(stdout: &str) -> ParsedStatus {
    let records: Vec<&str> = stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect();
    let mut parsed = ParsedStatus {
        branch_name: None,
        tracking_branch_name: None,
        head_ref_type: "branch",
        ahead: 0,
        behind: 0,
        entries: Vec::new(),
    };
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if let Some(header) = record.strip_prefix("# ") {
            if let Some(head) = header.strip_prefix("branch.head ") {
                if head == "(detached)" {
                    parsed.branch_name = None;
                    parsed.head_ref_type = "detached";
                } else {
                    parsed.branch_name = Some(head.to_owned());
                    parsed.head_ref_type = "branch";
                }
            } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
                parsed.tracking_branch_name = Some(upstream.to_owned());
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                (parsed.ahead, parsed.behind) = parse_ahead_behind(ab);
            }
            continue;
        }
        if let Some(path) = record.strip_prefix("? ") {
            parsed.entries.push(StatusEntry {
                path: normalize_git_path(path),
                original_path: None,
                kind: "added",
                x: None,
                y: Some("?".to_owned()),
                is_untracked: true,
                is_conflicted: false,
            });
            continue;
        }
        if record.starts_with("1 ") {
            let Some((xy, path)) = split_record(record, 6) else {
                continue;
            };
            let mut codes = xy.chars();
            let (x, y) = (codes.next().unwrap_or('.'), codes.next().unwrap_or('.'));
            parsed.entries.push(StatusEntry {
                path: normalize_git_path(&path),
                original_path: None,
                kind: kind_from_status_code(if x != '.' { x } else { y }),
                x: Some(x.to_string()),
                y: Some(y.to_string()),
                is_untracked: false,
                is_conflicted: false,
            });
            continue;
        }
        if record.starts_with("2 ") {
            let Some((xy, path)) = split_record(record, 7) else {
                continue;
            };
            // 重命名记录后紧跟原路径（TS 即使匹配失败也不消费下一条；匹配成功才 +1）。
            let original = records
                .get(index)
                .map(|original| normalize_git_path(original));
            index += 1;
            let mut codes = xy.chars();
            parsed.entries.push(StatusEntry {
                path: normalize_git_path(&path),
                original_path: original.filter(|path| !path.is_empty()),
                kind: "renamed",
                x: codes.next().map(String::from),
                y: codes.next().map(String::from),
                is_untracked: false,
                is_conflicted: false,
            });
            continue;
        }
        if record.starts_with("u ") {
            let Some((xy, path)) = split_record(record, 8) else {
                continue;
            };
            let mut codes = xy.chars();
            parsed.entries.push(StatusEntry {
                path: normalize_git_path(&path),
                original_path: None,
                kind: "modified",
                x: codes.next().map(String::from),
                y: codes.next().map(String::from),
                is_untracked: false,
                is_conflicted: true,
            });
        }
    }
    parsed
}

fn numstat_value(value: &str) -> i64 {
    if value == "-" {
        return 0;
    }
    js_parse_int(value).unwrap_or(0)
}

/// TS `parseNumstat`（`git diff --numstat -z`）。
pub fn parse_numstat(stdout: &str) -> NumstatMap {
    let records: Vec<&str> = stdout.split('\0').collect();
    let mut stats = NumstatMap::default();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.split('\t').collect();
        if fields.len() < 3 {
            continue;
        }
        let added = numstat_value(fields[0]);
        let removed = numstat_value(fields[1]);
        let path_field = fields[2..].join("\t");
        if !path_field.is_empty() {
            stats.set(
                normalize_git_path(&path_field),
                LineStat {
                    added,
                    removed,
                    kind: None,
                    original_path: None,
                },
            );
            continue;
        }
        let original = records.get(index).copied().unwrap_or_default();
        let renamed = records.get(index + 1).copied().unwrap_or_default();
        index += 2;
        if renamed.is_empty() {
            continue;
        }
        stats.set(
            normalize_git_path(renamed),
            LineStat {
                added,
                removed,
                kind: Some("renamed"),
                original_path: Some(normalize_git_path(original)),
            },
        );
    }
    stats
}

/// TS `inferKindFromNumstat`。
pub fn kind_from_numstat(stat: &LineStat) -> &'static str {
    if let Some(kind) = stat.kind {
        return kind;
    }
    if stat.added > 0 && stat.removed == 0 {
        return "added";
    }
    if stat.removed > 0 && stat.added == 0 {
        return "deleted";
    }
    "modified"
}

/// TS `parseRemoteList`。
pub fn parse_remote_list(stdout: &str) -> Vec<String> {
    stdout
        .replace("\r\n", "\n")
        .split('\n')
        .map(js_trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// TS `parseTrackingRemoteName`。
pub fn tracking_remote_name(tracking: Option<&str>) -> Option<String> {
    let remote = js_trim(tracking?.split('/').next().unwrap_or_default());
    (!remote.is_empty()).then(|| remote.to_owned())
}

/// TS `GitIndexEntry`（`git ls-files --stage -z`）。
pub struct IndexEntry {
    pub mode: String,
    pub object_hash: String,
    pub stage: String,
    pub path: String,
}

/// TS `parseGitIndexEntries`。
pub fn parse_index_entries(stdout: &str) -> anyhow::Result<Vec<IndexEntry>> {
    stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            let tab = record
                .find('\t')
                .ok_or_else(|| anyhow::anyhow!("Failed to parse staged Git index entry."))?;
            let head: Vec<&str> = js_trim(&record[..tab]).split_whitespace().collect();
            let path = normalize_git_path(&record[tab + 1..]);
            match (head.first(), head.get(1), head.get(2)) {
                (Some(mode), Some(hash), Some(stage)) if !path.is_empty() => Ok(IndexEntry {
                    mode: (*mode).to_owned(),
                    object_hash: (*hash).to_owned(),
                    stage: (*stage).to_owned(),
                    path,
                }),
                _ => anyhow::bail!("Failed to parse staged Git index entry."),
            }
        })
        .collect()
}

pub use super::parse_refs::{
    branch_mutation_issues, invalid_branch_name_issue, parse_branch_refs, parse_config_value,
    parse_graph_records,
};

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
