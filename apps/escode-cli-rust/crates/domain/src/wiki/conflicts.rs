//! 冲突检测（spec §6.6 / §1 规则 7）：只报告，不取舍。确定性规则：
//! 规范语句极性相反、Global 与 Project 规则冲突、待处理的来源更新、重复条目、断链、来源失效、版本不适用、
//! 条目声明的代码路径已不存在。
use super::entry::{Entry, Scope, Status, Validity};
use super::similarity::similar_pairs;
use super::text::tokenize;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConflictParty {
    pub id: String,
    pub scope: Scope,
    pub status: String,
    pub authority: String,
    pub updated_at: String,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    pub kind: &'static str,
    pub parties: Vec<ConflictParty>,
    pub evidence: String,
    pub suggestion: String,
}

const POSITIVE: &[&str] = &[
    "must", "always", "required", "shall", "必须", "应当", "务必", "一律",
];
const NEGATIVE: &[&str] = &[
    "must not",
    "never",
    "shall not",
    "do not",
    "don't",
    "禁止",
    "不得",
    "不允许",
    "严禁",
    "不要",
    "不能",
];

fn party(entry: &Entry) -> ConflictParty {
    ConflictParty {
        id: entry.meta.id.clone(),
        scope: entry.meta.scope,
        status: entry.meta.status.as_str().into(),
        authority: entry.meta.authority.as_str().into(),
        updated_at: entry.meta.updated_at.clone(),
        sources: entry
            .meta
            .sources
            .iter()
            .map(|s| format!("{} → {}", s.source_path, s.locator.value))
            .collect(),
    }
}

/// 规范句：含规范词的句子，返回（极性, 去掉规范词后的主题词集合, 原句）。
fn normative_clauses(body: &str) -> Vec<(bool, BTreeSet<String>, String)> {
    let noise: BTreeSet<String> = POSITIVE
        .iter()
        .chain(NEGATIVE)
        .flat_map(|w| tokenize(w))
        .collect();
    body.split(['.', '。', '\n', ';', '；', '!', '！'])
        .filter_map(|sentence| {
            let lower = sentence.to_lowercase();
            let negative = NEGATIVE.iter().any(|w| lower.contains(w));
            let positive = !negative && POSITIVE.iter().any(|w| lower.contains(w));
            if !negative && !positive {
                return None;
            }
            let topic: BTreeSet<String> = tokenize(sentence)
                .into_iter()
                .filter(|t| !noise.contains(t))
                .collect();
            (topic.len() >= 2).then(|| (positive, topic, sentence.trim().to_owned()))
        })
        .collect()
}

fn active(entry: &Entry) -> bool {
    matches!(entry.meta.status, Status::Approved | Status::NeedsReview)
}

/// `entries` 为同一检索可见范围（global + 当前项目）的全部条目。
/// `existing_paths(glob)` 判断项目里是否仍有匹配文件；`project_version` 为项目版本（可缺）。
pub fn detect(
    entries: &[Entry],
    existing_paths: &dyn Fn(&str) -> bool,
    project_version: Option<&str>,
) -> Vec<Conflict> {
    let mut out = vec![];
    let ids: HashMap<&str, &Entry> = entries.iter().map(|e| (e.meta.id.as_str(), e)).collect();
    let clauses: Vec<_> = entries.iter().map(|e| normative_clauses(&e.body)).collect();
    let bodies: Vec<BTreeSet<String>> = entries
        .iter()
        .map(|e| tokenize(&e.body).into_iter().collect())
        .collect();
    // 修复原因：原实现对全部条目两两做集合 Jaccard（O(n²) 次集合运算），1500 条目时 status/graph
    // 需要数十秒；改为前缀过滤生成候选后精确校验（similarity.rs），判定结果与两两比较一致。
    let items: Vec<(usize, usize)> = clauses
        .iter()
        .enumerate()
        .flat_map(|(entry, list)| (0..list.len()).map(move |clause| (entry, clause)))
        .collect();
    let clause_sets: Vec<&BTreeSet<String>> =
        items.iter().map(|&(e, c)| &clauses[e][c].1).collect();
    let mut contradictions: BTreeMap<(usize, usize), (usize, usize)> = BTreeMap::new();
    let opposite = |x: usize, y: usize| {
        let ((ex, cx), (ey, cy)) = (items[x], items[y]);
        ex != ey
            && active(&entries[ex])
            && active(&entries[ey])
            && clauses[ex][cx].0 != clauses[ey][cy].0
    };
    for (x, y) in similar_pairs(&clause_sets, 0.5, &opposite) {
        let ((mut ei, mut ci), (mut ej, mut cj)) = (items[x], items[y]);
        if ei > ej {
            (ei, ej, ci, cj) = (ej, ei, cj, ci);
        }
        // 与原先「i 的子句顺序优先、再取 j 的首个匹配」一致：保留字典序最小的子句对作为证据。
        let slot = contradictions.entry((ei, ej)).or_insert((ci, cj));
        *slot = (*slot).min((ci, cj));
    }
    let body_sets: Vec<&BTreeSet<String>> = bodies.iter().collect();
    let duplicate = |a: usize, b: usize| {
        active(&entries[a])
            && active(&entries[b])
            && bodies[a].len() >= 8
            && !contradictions.contains_key(&(a, b))
    };
    let duplicates: BTreeSet<(usize, usize)> = similar_pairs(&body_sets, 0.9, &duplicate)
        .into_iter()
        .collect();
    let pairs: BTreeSet<(usize, usize)> =
        contradictions.keys().copied().chain(duplicates).collect();
    for (i, j) in pairs {
        let (a, b) = (&entries[i], &entries[j]);
        if let Some(&(ci, cj)) = contradictions.get(&(i, j)) {
            let (ca, cb) = (&clauses[i][ci], &clauses[j][cj]);
            let cross = a.meta.scope != b.meta.scope;
            out.push(Conflict {
                kind: if cross { "global_project_conflict" } else { "normative_contradiction" },
                parties: vec![party(a), party(b)],
                evidence: format!("\"{}\" vs \"{}\"", ca.2, cb.2),
                suggestion: if cross {
                    "Decide whether the project rule intentionally overrides the global one; record the decision (e.g. supersedes) or deprecate one entry.".into()
                } else {
                    "Review both entries with their sources and deprecate or supersede the outdated one.".into()
                },
            });
        } else {
            out.push(Conflict {
                kind: "duplicate",
                parties: vec![party(a), party(b)],
                evidence: "bodies are at least 90% identical".into(),
                suggestion: "Keep one entry and deprecate the other with superseded_by.".into(),
            });
        }
    }
    for entry in entries {
        let refs = entry
            .meta
            .related
            .iter()
            .chain(&entry.meta.supersedes)
            .chain(&entry.meta.superseded_by);
        for target in refs {
            if !ids.contains_key(target.as_str()) {
                out.push(single(
                    entry,
                    "broken_reference",
                    format!("references missing entry '{target}'"),
                    "Fix or remove the reference.",
                ));
            }
        }
        if entry.meta.status == Status::Proposal
            && let Some(target) = entry.meta.supersedes.as_deref().and_then(|t| ids.get(t))
            && target.meta.status == Status::Approved
        {
            out.push(Conflict {
                kind: "pending_update",
                parties: vec![party(entry), party(target)],
                evidence: "a newer source revision proposes changes to an approved entry".into(),
                suggestion: "Approve the proposal (it supersedes the approved entry) or reject it."
                    .into(),
            });
        }
        if active(entry)
            && matches!(
                entry.meta.validity.state,
                Validity::Stale | Validity::BrokenSource
            )
        {
            let reason = entry.meta.validity.reason.clone().unwrap_or_default();
            out.push(single(
                entry,
                "stale_source",
                format!("validity {}: {reason}", entry.meta.validity.state.as_str()),
                "Re-verify the entry against its source and update or deprecate it.",
            ));
        }
        if active(entry) {
            for glob in &entry.meta.applies_to.paths {
                if !existing_paths(glob) {
                    out.push(single(
                        entry,
                        "code_mismatch",
                        format!("applies_to path '{glob}' matches no file in the project"),
                        "The code may have moved; update applies_to or re-verify the entry.",
                    ));
                }
            }
            if let (Some(range), Some(version)) =
                (entry.meta.applies_to.versions.as_deref(), project_version)
                && !version_applies(range, version)
            {
                out.push(single(
                    entry,
                    "version_mismatch",
                    format!("entry applies to {range}, project is {version}"),
                    "Confirm the entry still applies to this version.",
                ));
            }
        }
    }
    out
}

fn single(entry: &Entry, kind: &'static str, evidence: String, suggestion: &str) -> Conflict {
    Conflict {
        kind,
        parties: vec![party(entry)],
        evidence,
        suggestion: suggestion.into(),
    }
}

/// 版本适用性：支持 `1.x`、`>=2`、`>=2.3`、`<3`、空格或逗号分隔的多个条件（全部满足）。只比较主次版本。
pub fn version_applies(range: &str, version: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64)> {
        let mut parts = v.trim().trim_start_matches('v').split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
        Some((major, minor))
    };
    let Some(current) = parse(version) else {
        return true;
    };
    range
        .split([',', ' '])
        .filter(|c| !c.is_empty())
        .all(|cond| {
            if let Some(major) = cond.strip_suffix(".x") {
                return major
                    .trim_start_matches('v')
                    .parse::<u64>()
                    .is_ok_and(|m| m == current.0);
            }
            let (op, rest) = ["<=", ">=", "<", ">", "="]
                .iter()
                .find_map(|op| cond.strip_prefix(op).map(|r| (*op, r)))
                .unwrap_or(("=", cond));
            let Some(target) = parse(rest) else {
                return true;
            };
            match op {
                "<=" => current <= target,
                ">=" => current >= target,
                "<" => current < target,
                ">" => current > target,
                _ => current.0 == target.0 && (!rest.contains('.') || current.1 == target.1),
            }
        })
}
