//! git 引用/提交图/配置/分支错误的输出解析（TS `gitCliRepo.ts` / `gitCliHelpers.ts` 同名函数）。

use serde_json::{Value, json};

use super::parse::js_parse_int;
use super::paths::{js_trim, js_trim_end, normalize_git_path};

/// (name, isCurrent, upstream, commitHash, commitTimestampMs)。
type BranchRow = (String, bool, Option<String>, Option<String>, Option<i64>);

/// TS `parseBranchRefRecords`（排序比较器由调用方注入：`localeCompare` 走 ICU）。
pub fn parse_branch_refs(
    stdout: &str,
    current: Option<&str>,
    locale_compare: impl Fn(&str, &str) -> std::cmp::Ordering,
) -> Vec<Value> {
    let mut branches: Vec<BranchRow> = stdout
        .replace("\r\n", "\n")
        .split('\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let name = fields.next().filter(|name| !name.is_empty())?.to_owned();
            let upstream = fields
                .next()
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let hash = fields
                .next()
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let timestamp = fields
                .next()
                .filter(|value| !value.is_empty())
                .and_then(js_parse_int)
                .map(|seconds| seconds * 1000);
            Some((
                name.clone(),
                Some(name.as_str()) == current,
                upstream,
                hash,
                timestamp,
            ))
        })
        .collect();
    branches.sort_by(|left, right| {
        if left.1 != right.1 {
            return if left.1 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let left_time = left.4.map_or(f64::NEG_INFINITY, |value| value as f64);
        let right_time = right.4.map_or(f64::NEG_INFINITY, |value| value as f64);
        if left_time != right_time {
            return right_time
                .partial_cmp(&left_time)
                .unwrap_or(std::cmp::Ordering::Equal);
        }
        locale_compare(&left.0, &right.0)
    });
    branches
        .into_iter()
        .map(|(name, is_current, upstream, hash, timestamp)| {
            json!({
                "name": name,
                "isCurrent": is_current,
                "upstreamName": upstream,
                "commitHash": hash,
                "commitTimestampMs": timestamp,
            })
        })
        .collect()
}

fn graph_ref(name: &str, kind: &str) -> Value {
    json!({"name": name, "kind": kind})
}

fn parse_decoration_ref(raw: &str) -> Option<Value> {
    let reference = js_trim(raw);
    if reference.is_empty() {
        return None;
    }
    if reference == "HEAD" {
        return Some(graph_ref("HEAD", "head"));
    }
    if let Some(tag) = reference.strip_prefix("tag: ") {
        let tag = js_trim(tag);
        let name = tag.strip_prefix("refs/tags/").unwrap_or(tag);
        return (!name.is_empty()).then(|| graph_ref(name, "tag"));
    }
    for (prefix, kind) in [
        ("refs/heads/", "branch"),
        ("refs/remotes/", "remote"),
        ("refs/tags/", "tag"),
    ] {
        if let Some(name) = reference.strip_prefix(prefix) {
            return (!name.is_empty()).then(|| graph_ref(name, kind));
        }
    }
    Some(graph_ref(
        reference,
        if reference.contains('/') {
            "remote"
        } else {
            "branch"
        },
    ))
}

fn add_ref(refs: &mut Vec<Value>, reference: Value) {
    if !refs.iter().any(|existing| {
        existing["kind"] == reference["kind"] && existing["name"] == reference["name"]
    }) {
        refs.push(reference);
    }
}

/// TS `parseGitGraphRefs`。
fn parse_graph_refs(decorations: &str) -> Vec<Value> {
    let mut refs = Vec::new();
    for raw in decorations.split(',') {
        let decoration = js_trim(raw);
        if decoration.is_empty() {
            continue;
        }
        if let Some(pointed) = decoration.strip_prefix("HEAD -> ") {
            add_ref(&mut refs, graph_ref("HEAD", "head"));
            if let Some(reference) = parse_decoration_ref(pointed) {
                add_ref(&mut refs, reference);
            }
            continue;
        }
        if let Some(reference) = parse_decoration_ref(decoration) {
            add_ref(&mut refs, reference);
        }
    }
    refs
}

/// TS `parseGitGraphRecords`（`%H%x00%P%x00%an%x00%at%x00%s%x00%D%x1e`）。
pub fn parse_graph_records(stdout: &str) -> Vec<Value> {
    stdout
        .split('\u{1e}')
        .map(js_trim)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.split('\0').collect();
            let hash = fields.first().filter(|hash| !hash.is_empty())?;
            let parents: Vec<&str> = fields
                .get(1)
                .map(|parents| {
                    parents
                        .split(' ')
                        .filter(|parent| !parent.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let timestamp = fields
                .get(3)
                .filter(|value| !value.is_empty())
                .and_then(|value| js_parse_int(value))
                .map(|seconds| seconds * 1000);
            Some(json!({
                "hash": hash,
                "parents": parents,
                "refs": parse_graph_refs(fields.get(5).copied().unwrap_or_default()),
                "subject": fields.get(4).copied().unwrap_or_default(),
                "authorName": fields.get(2).filter(|name| !name.is_empty()),
                "authoredAtMs": timestamp,
            }))
        })
        .collect()
}

/// TS `parseGitConfigValue`（`git config --show-scope --show-origin --get`）：`(scope, source, value)`。
pub fn parse_config_value(
    output: &super::command::GitOutput,
) -> anyhow::Result<(Option<String>, Option<String>, Option<String>)> {
    if output.exit_code == Some(1) {
        return Ok((None, None, None));
    }
    super::command::ensure_succeeded("git config", output, &[0])?;
    let line = output
        .stdout
        .strip_suffix("\r\n")
        .or_else(|| output.stdout.strip_suffix('\n'))
        .unwrap_or(&output.stdout);
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return Ok((None, None, (!line.is_empty()).then(|| line.to_owned())));
    }
    let value = parts[2..].join("\t");
    Ok((
        Some(parts[0].to_owned()),
        Some(parts[1].to_owned()),
        (!value.is_empty()).then_some(value),
    ))
}

/// TS `toInvalidBranchNameIssue`。
pub fn invalid_branch_name_issue(detail: Option<&str>) -> Value {
    let detail = detail.map(js_trim).filter(|detail| !detail.is_empty());
    json!({"code": "invalid-branch-name", "message": "Branch name is invalid.", "detail": detail})
}

fn normalized_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|line| js_trim_end(line).to_owned())
        .collect()
}

/// TS `extractIndentedPaths`：命中标题行之后的缩进行，遇到空行跳过、非缩进行停止。
fn extract_indented_paths(lines: &[String], header: impl Fn(&str) -> bool) -> Vec<String> {
    let Some(start) = lines.iter().position(|line| header(&line.to_lowercase())) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for line in &lines[start + 1..] {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(|character: char| character.is_whitespace() || character == '\u{feff}')
        {
            break;
        }
        let value = js_trim(line);
        if !value.is_empty() {
            paths.push(normalize_git_path(value));
        }
    }
    paths
}

/// TS `parseGitBranchMutationIssues`。
pub fn branch_mutation_issues(output: &super::command::GitOutput) -> Vec<Value> {
    let stderr = js_trim(&output.stderr);
    let stdout = js_trim(&output.stdout);
    let detail = if !stderr.is_empty() {
        Some(stderr.to_owned())
    } else if !stdout.is_empty() {
        Some(stdout.to_owned())
    } else {
        None
    };
    let lines = normalized_lines(detail.as_deref().unwrap_or_default());
    let lower = detail.as_deref().unwrap_or_default().to_lowercase();
    let overwritten = |suffix: &'static str| {
        move |line: &str| {
            ["checkout", "switch"]
                .iter()
                .any(|verb| line.contains(&format!("{suffix}{verb}")))
        }
    };
    let issue =
        |code: &str, message: &str| json!({"code": code, "message": message, "detail": detail});
    let tracked = extract_indented_paths(
        &lines,
        overwritten("your local changes to the following files would be overwritten by "),
    );
    if !tracked.is_empty() {
        let mut value = issue(
            "tracked-changes-would-be-overwritten",
            "Tracked changes would be overwritten by switching branches.",
        );
        value["paths"] = json!(tracked);
        return vec![value];
    }
    let untracked = extract_indented_paths(
        &lines,
        overwritten("the following untracked working tree files would be overwritten by "),
    );
    if !untracked.is_empty() {
        let mut value = issue(
            "untracked-changes-would-be-overwritten",
            "Untracked files would be overwritten by switching branches.",
        );
        value["paths"] = json!(untracked);
        return vec![value];
    }
    if lower.contains("already exists") {
        return vec![issue("branch-already-exists", "Branch already exists.")];
    }
    if lower.contains("invalid reference:") {
        return vec![issue(
            "target-branch-not-found",
            "Target branch was not found.",
        )];
    }
    if lower.contains("is already used by worktree at") {
        return vec![issue(
            "branch-in-other-worktree",
            "Branch is already checked out in another worktree.",
        )];
    }
    if lower.contains("resolve your current index first") {
        return vec![issue(
            "conflicts-present",
            "Repository still has unresolved conflicts.",
        )];
    }
    let in_progress = [
        "merging",
        "rebasing",
        "cherry-picking",
        "reverting",
        "bisecting",
    ]
    .iter()
    .any(|state| lower.contains(&format!("cannot switch branch while {state}")))
        || lower.contains("you have not concluded your merge")
        || lower.contains("rebase in progress");
    if in_progress {
        return vec![issue(
            "operation-in-progress",
            "Another Git operation is still in progress.",
        )];
    }
    vec![issue(
        "unknown",
        "Git could not complete the branch operation.",
    )]
}
