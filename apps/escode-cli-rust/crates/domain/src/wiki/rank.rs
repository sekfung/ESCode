//! 检索打分与上下文选择（spec §8）。一次检索可跨多个索引（global + 当前项目）；
//! 每个索引单独算 BM25，再统一乘以类型/状态/权威/作用域/有效性权重并归一化。
use super::entry::{Kind, Scope, Status, Validity, authority_weight};
use super::index::{DocStats, Index};
use super::text::tokenize;
use super::workflow::{Workflow, kind_weight};
use std::collections::BTreeSet;

const K1: f32 = 1.2;
const B: f32 = 0.75;
const PATH_BOOST: f64 = 2.0;
/// 归一化：1 - e^(-score/NORM)，单个强字段命中约 0.5，正文单词命中约 0.3。
const NORM: f64 = 4.0;

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub terms: Vec<String>,
    pub workflow: Option<Workflow>,
    pub paths: Vec<String>,
    pub kinds: Option<Vec<Kind>>,
    pub include_proposals: bool,
    pub include_retired: bool,
}

impl Query {
    pub fn new(text: &str, workflow: Option<Workflow>, paths: Vec<String>) -> Self {
        let mut terms = tokenize(text);
        // 路径段也是检索词（src/api/error.rs → api、error），让模块相关知识浮上来。
        for path in &paths {
            terms.extend(tokenize(&path.replace(['/', '\\', '.', '_', '-'], " ")));
        }
        let mut seen = BTreeSet::new();
        terms.retain(|t| seen.insert(t.clone()));
        Query {
            terms,
            workflow,
            paths,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: String,
    pub scope: Scope,
    pub raw: f64,
    pub score: f64,
    pub strong_matches: usize,
    pub body_matches: usize,
}

pub fn status_weight(status: Status) -> f64 {
    match status {
        Status::Approved => 1.0,
        Status::NeedsReview => 0.6,
        Status::Proposal => 0.5,
        _ => 0.3,
    }
}

pub fn validity_weight(validity: Validity) -> f64 {
    match validity {
        Validity::Valid | Validity::Unverified => 1.0,
        Validity::Stale => 0.6,
        Validity::BrokenSource => 0.5,
    }
}

fn allowed(doc: &DocStats, query: &Query) -> bool {
    let retired = matches!(
        doc.status,
        Status::Deprecated | Status::Superseded | Status::Rejected
    );
    if retired && !query.include_retired {
        return false;
    }
    if doc.status == Status::Proposal && !query.include_proposals {
        return false;
    }
    query
        .kinds
        .as_ref()
        .is_none_or(|kinds| kinds.contains(&doc.kind))
}

/// 在单个索引内检索。`expected` 是该索引应有的作用域（结果阶段的作用域校验在调用方完成）。
pub fn search(index: &Index, query: &Query) -> Vec<Hit> {
    let n = index.docs.len() as f32;
    if n == 0.0 || query.terms.is_empty() {
        return vec![];
    }
    let avg = index.average_length();
    let mut acc: std::collections::HashMap<&str, (f32, usize, usize)> = Default::default();
    for term in &query.terms {
        let Some(list) = index.postings.get(term) else {
            continue;
        };
        let df = list.len() as f32;
        // 平滑 IDF：小知识库（个位数条目）里经典 BM25 的 IDF 趋近 0，标题完全命中也过不了门槛；
        // 用 (N+1)/(df+0.5) 保底，同时保留「越常见权重越低」的单调性。
        let idf = (1.0 + (n + 1.0) / (df + 0.5)).ln();
        for (id, tf) in list {
            let Some(doc) = index.docs.get(id) else {
                continue;
            };
            let norm = tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * doc.length / avg));
            let slot = acc.entry(id.as_str()).or_default();
            slot.0 += idf * norm;
            if doc.strong_terms.binary_search(term).is_ok() {
                slot.1 += 1;
            } else {
                slot.2 += 1;
            }
        }
    }
    let workflow = query.workflow.unwrap_or(Workflow::General);
    let mut hits: Vec<Hit> = acc
        .into_iter()
        .filter_map(|(id, (bm25, strong, body))| {
            let doc = index.docs.get(id)?;
            if !allowed(doc, query) {
                return None;
            }
            let mut raw = f64::from(bm25)
                * kind_weight(workflow, doc.kind)
                * status_weight(doc.status)
                * authority_weight(doc.authority)
                * validity_weight(doc.validity)
                * if doc.scope == Scope::Project {
                    1.1
                } else {
                    1.0
                };
            if doc
                .applies_to
                .iter()
                .any(|g| query.paths.iter().any(|p| glob_match(g, p)))
            {
                raw += PATH_BOOST;
            }
            Some(Hit {
                id: id.to_owned(),
                scope: doc.scope,
                raw,
                score: 1.0 - (-raw / NORM).exp(),
                strong_matches: strong,
                body_matches: body,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.raw.total_cmp(&a.raw).then_with(|| a.id.cmp(&b.id)));
    hits
}

/// 相关性门槛：分数达标，且至少命中一个标题/标签词或两个不同正文词。
pub fn passes_gate(hit: &Hit, min_score: f64) -> bool {
    hit.score >= min_score && (hit.strong_matches >= 1 || hit.body_matches >= 2)
}

/// 极简 glob：`**` 跨目录、`*` 不跨 `/`、`?` 单字符；路径分隔符统一为 `/`。
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    let path = path.trim_start_matches("./");
    fn go(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(b'*'), _) if p.get(1) == Some(&b'*') => {
                let rest = p[2..].strip_prefix(b"/").unwrap_or(&p[2..]);
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            (Some(b'*'), _) => (0..=s.len())
                .take_while(|&i| i == 0 || s[i - 1] != b'/')
                .any(|i| go(&p[1..], &s[i..])),
            (Some(b'?'), Some(c)) if *c != b'/' => go(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
            _ => false,
        }
    }
    go(pattern.trim_start_matches("./").as_bytes(), path.as_bytes())
        || (!pattern.contains('/')
            && path
                .rsplit('/')
                .next()
                .is_some_and(|name| go(pattern.as_bytes(), name.as_bytes())))
}

/// 长条目只取与查询最相关的段落（保持原顺序），不超过 `max_tokens`。
pub fn best_excerpt(body: &str, terms: &[String], max_tokens: usize) -> (String, bool) {
    use super::text::estimate_tokens;
    if estimate_tokens(body) <= max_tokens {
        return (body.trim().to_owned(), false);
    }
    let paragraphs: Vec<&str> = body
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .collect();
    let mut scored: Vec<(usize, usize)> = paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let tokens: BTreeSet<String> = tokenize(p).into_iter().collect();
            (i, terms.iter().filter(|t| tokens.contains(*t)).count())
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut chosen = vec![];
    let mut used = 0;
    for (i, _) in scored {
        let cost = estimate_tokens(paragraphs[i]);
        if used + cost > max_tokens {
            if chosen.is_empty() {
                let chars = max_tokens * 3;
                return (super::text::truncate_chars(paragraphs[i], chars), true);
            }
            continue;
        }
        used += cost;
        chosen.push(i);
    }
    chosen.sort();
    (
        chosen
            .iter()
            .map(|&i| paragraphs[i])
            .collect::<Vec<_>>()
            .join("\n\n"),
        true,
    )
}
