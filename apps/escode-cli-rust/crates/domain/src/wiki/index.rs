//! 关键词倒排索引（spec §8）：派生数据，可随时由条目重建；`fingerprint` 与条目集合不符即视为失效。
use super::entry::{Authority, Entry, Kind, Scope, Status, Validity};
use super::text::{sha256_hex, tokenize};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const INDEX_FORMAT: u32 = 1;
const TITLE_WEIGHT: f32 = 3.0;
const TAG_WEIGHT: f32 = 2.5;
const HEADING_WEIGHT: f32 = 1.5;
const BODY_WEIGHT: f32 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocStats {
    pub title: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub kind: Kind,
    pub status: Status,
    pub authority: Authority,
    pub validity: Validity,
    pub updated_at: String,
    pub length: f32,
    /// 标题与标签词（门槛判断：至少命中一个强字段词）。
    pub strong_terms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applies_to: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub format: u32,
    /// 每次变更 +1；检索缓存键的一部分。
    pub version: u64,
    pub fingerprint: String,
    pub docs: BTreeMap<String, DocStats>,
    pub postings: BTreeMap<String, Vec<(String, f32)>>,
}

/// 条目集合指纹：`id@updated_at` 排序后哈希。条目文件被手工改动或写入中途崩溃时与索引不符。
pub fn fingerprint<'a>(versions: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut items: Vec<String> = versions.map(|(id, v)| format!("{id}@{v}")).collect();
    items.sort();
    sha256_hex(items.join("\n").as_bytes())[..32].to_owned()
}

fn weighted_terms(entry: &Entry) -> (HashMap<String, f32>, Vec<String>) {
    let mut terms: HashMap<String, f32> = HashMap::new();
    let mut strong = vec![];
    let mut add = |text: &str, weight: f32, strong_out: Option<&mut Vec<String>>| {
        let tokens = tokenize(text);
        if let Some(out) = strong_out {
            out.extend(tokens.iter().cloned());
        }
        for token in tokens {
            *terms.entry(token).or_default() += weight;
        }
    };
    add(&entry.meta.title, TITLE_WEIGHT, Some(&mut strong));
    add(&entry.meta.tags.join(" "), TAG_WEIGHT, Some(&mut strong));
    for line in entry.body.lines() {
        if line.trim_start().starts_with('#') {
            add(line, HEADING_WEIGHT, None);
        } else {
            add(line, BODY_WEIGHT, None);
        }
    }
    strong.sort();
    strong.dedup();
    (terms, strong)
}

impl Index {
    pub fn build<'a>(entries: impl Iterator<Item = &'a Entry>) -> Self {
        let mut index = Index {
            format: INDEX_FORMAT,
            ..Default::default()
        };
        for entry in entries {
            index.insert(entry);
        }
        index.refresh_fingerprint();
        index
    }

    pub fn refresh_fingerprint(&mut self) {
        let docs = &self.docs;
        self.fingerprint = fingerprint(
            docs.iter()
                .map(|(id, d)| (id.as_str(), d.updated_at.as_str())),
        );
    }

    fn insert(&mut self, entry: &Entry) {
        let (terms, strong_terms) = weighted_terms(entry);
        let length = terms.values().sum::<f32>();
        let id = entry.meta.id.clone();
        for (term, weight) in terms {
            self.postings
                .entry(term)
                .or_default()
                .push((id.clone(), weight));
        }
        self.docs.insert(
            id,
            DocStats {
                title: entry.meta.title.clone(),
                scope: entry.meta.scope,
                project_id: entry.meta.project_id.clone(),
                kind: entry.meta.kind,
                status: entry.meta.status,
                authority: entry.meta.authority,
                validity: entry.meta.validity.state,
                updated_at: entry.meta.updated_at.clone(),
                length,
                strong_terms,
                applies_to: entry.meta.applies_to.paths.clone(),
            },
        );
    }

    /// 增量：删除旧 posting 后插入新版本（只动受影响条目）。
    pub fn upsert(&mut self, entry: &Entry) {
        self.remove_postings(&entry.meta.id);
        self.insert(entry);
        self.version += 1;
        self.refresh_fingerprint();
    }

    pub fn remove(&mut self, id: &str) {
        self.remove_postings(id);
        self.docs.remove(id);
        self.version += 1;
        self.refresh_fingerprint();
    }

    fn remove_postings(&mut self, id: &str) {
        if !self.docs.contains_key(id) {
            return;
        }
        self.postings.retain(|_, list| {
            list.retain(|(doc, _)| doc != id);
            !list.is_empty()
        });
    }

    pub fn average_length(&self) -> f32 {
        if self.docs.is_empty() {
            return 1.0;
        }
        (self.docs.values().map(|d| d.length).sum::<f32>() / self.docs.len() as f32).max(1.0)
    }
}
