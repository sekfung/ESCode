//! 检索与上下文组装（spec §8、§9）：只打开 global + 当前项目两个快照；查询阶段与结果阶段都做作用域校验；
//! 门槛 → 状态 → 去重 → 预算 依次过滤并逐项计数；缓存键含项目、两个索引版本、配置指纹、工作流、查询与去重集合。
use super::service::{Snapshot, WikiService, now};
use anyhow::Result;
use escode_cli_core_api::{WikiInjected, WikiTurnRetrieval};
use escode_cli_domain::wiki::entry::{Entry, Scope};
use escode_cli_domain::wiki::rank::{self, Hit, Query, best_excerpt, passes_gate};
use escode_cli_domain::wiki::reminder::{self, InjectedEntry};
use escode_cli_domain::wiki::text::{estimate_tokens, sha256_hex};
use escode_cli_domain::wiki::workflow::{self, Workflow};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

const CACHE_SIZE: usize = 64;

#[derive(Default)]
pub struct TurnCache {
    items: VecDeque<(String, WikiTurnRetrieval)>,
}

impl TurnCache {
    pub fn clear(&mut self) {
        self.items.clear();
    }
    fn get(&self, key: &str) -> Option<WikiTurnRetrieval> {
        self.items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
    fn put(&mut self, key: String, value: WikiTurnRetrieval) {
        if self.items.len() >= CACHE_SIZE {
            self.items.pop_front();
        }
        self.items.push_back((key, value));
    }
}

pub struct Scored<'a> {
    pub hit: Hit,
    pub entry: &'a Entry,
}

/// 结果阶段的作用域校验：条目必须属于打开它的存储（防御性：手工放错目录的条目不会越界）。
fn in_scope(entry: &Entry, snapshot: &Snapshot) -> bool {
    entry.meta.scope == snapshot.store.scope && entry.meta.project_id == snapshot.store.project_id
}

pub fn search_snapshots<'a>(
    snapshots: &'a [Snapshot],
    query: &Query,
    filtered: &mut BTreeMap<&'static str, usize>,
) -> Vec<Scored<'a>> {
    let mut out = vec![];
    for snapshot in snapshots {
        for hit in rank::search(&snapshot.index, query) {
            let Some(entry) = snapshot.entries.get(&hit.id) else {
                *filtered.entry("index_stale").or_default() += 1;
                continue;
            };
            if !in_scope(entry, snapshot) {
                *filtered.entry("scope_violation").or_default() += 1;
                continue;
            }
            out.push(Scored { hit, entry });
        }
    }
    out.sort_by(|a, b| {
        b.hit
            .raw
            .total_cmp(&a.hit.raw)
            .then_with(|| a.hit.id.cmp(&b.hit.id))
    });
    out
}

pub fn source_label(entry: &Entry) -> Option<String> {
    entry.meta.sources.first().map(|s| {
        let file = s
            .source_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&s.source_path);
        format!("{file} → {}", s.locator.value)
    })
}

/// 用户输入里提到的文件路径（含 `/` 或常见源码扩展名的词），用于 applies_to 匹配与模块词。
pub fn mentioned_paths(prompt: &str) -> Vec<String> {
    const EXT: [&str; 14] = [
        ".rs", ".ts", ".tsx", ".js", ".py", ".go", ".java", ".kt", ".swift", ".c", ".cpp", ".h",
        ".md", ".json",
    ];
    prompt
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | ',' | '，' | '：' | ':')
        })
        .map(|w| w.trim_matches(|c: char| matches!(c, '.' | '。')))
        .filter(|w| {
            w.len() > 2
                && (w.contains('/') || EXT.iter().any(|e| w.ends_with(e)))
                && !w.contains("://")
        })
        .map(str::to_owned)
        .take(32)
        .collect()
}

impl WikiService {
    pub async fn open_snapshots(&self) -> Result<Vec<Snapshot>> {
        let settings = self.settings().await;
        let (project, global) = self.stores(&settings).await;
        Ok(vec![
            self.snapshot(&project).await?,
            self.snapshot(&global).await?,
        ])
    }

    pub async fn retrieve(
        &self,
        session: &str,
        prompt: &str,
        already: &HashMap<String, String>,
    ) -> Result<WikiTurnRetrieval> {
        let started = Instant::now();
        let settings = self.settings().await;
        let workflow = workflow::classify(prompt);
        let disabled = |status| WikiTurnRetrieval {
            status,
            workflow: workflow.as_str(),
            body: None,
            injected: vec![],
            retrieved: 0,
            filtered: 0,
            duration_ms: 0,
            index_versions: String::new(),
        };
        if !settings.config.enabled || !settings.config.auto_inject {
            return Ok(disabled("disabled"));
        }
        let snapshots = self.open_snapshots().await?;
        let versions = format!(
            "project:{},global:{}",
            snapshots[0].index.version, snapshots[1].index.version
        );
        if snapshots.iter().all(|s| s.entries.is_empty()) {
            return Ok(WikiTurnRetrieval {
                index_versions: versions,
                ..disabled("empty")
            });
        }
        let mut paths = mentioned_paths(prompt);
        if workflow == Workflow::Review {
            paths.extend(super::changes::changed_files(&self.project().await.root).await);
        }
        let mut seen: Vec<(&String, &String)> = already.iter().collect();
        seen.sort();
        let project = self.project().await;
        let key = sha256_hex(
            format!(
                "{}|{versions}|{}|{}|{prompt}|{seen:?}|{paths:?}",
                project.id,
                settings.config.fingerprint(),
                workflow.as_str()
            )
            .as_bytes(),
        );
        if let Some(cached) = self.turn_cache.lock().await.get(&key) {
            return Ok(cached);
        }
        let mut query = Query::new(prompt, Some(workflow), paths);
        // 提案参与打分但在选择阶段以 not_injectable 过滤：审计能看到「检索到但未注入」的原因。
        query.include_proposals = true;
        let mut filtered: BTreeMap<&'static str, usize> = BTreeMap::new();
        let scored = search_snapshots(&snapshots, &query, &mut filtered);
        let retrieved = scored.len();
        let retrieval = &settings.config.retrieval;
        let min_score = retrieval.min_score * workflow::threshold_factor(workflow);
        let mut chosen: Vec<(&Scored<'_>, String, bool, usize)> = vec![];
        let mut used = 0usize;
        for item in &scored {
            let reason = if !passes_gate(&item.hit, min_score) {
                Some("below_threshold")
            } else if !item.entry.injectable() {
                Some("not_injectable")
            } else if already
                .get(&item.entry.meta.id)
                .is_some_and(|v| v == item.entry.version())
            {
                Some("already_injected")
            } else if chosen.len() >= retrieval.max_results {
                Some("max_results")
            } else {
                None
            };
            if let Some(reason) = reason {
                *filtered.entry(reason).or_default() += 1;
                continue;
            }
            let (text, truncated) =
                best_excerpt(&item.entry.body, &query.terms, retrieval.max_entry_tokens);
            let tokens = estimate_tokens(&text) + 40;
            if used + tokens > retrieval.max_injected_tokens {
                *filtered.entry("budget").or_default() += 1;
                continue;
            }
            used += tokens;
            chosen.push((item, text, truncated, tokens));
        }
        let filtered_count: usize = filtered.values().sum();
        let mut result = WikiTurnRetrieval {
            status: if chosen.is_empty() {
                "no_match"
            } else {
                "injected"
            },
            workflow: workflow.as_str(),
            body: None,
            injected: chosen
                .iter()
                .map(|(s, _, _, tokens)| WikiInjected {
                    id: s.entry.meta.id.clone(),
                    version: s.entry.version().to_owned(),
                    tokens: *tokens,
                })
                .collect(),
            retrieved,
            filtered: filtered_count,
            duration_ms: 0,
            index_versions: versions,
        };
        if !chosen.is_empty() {
            let kinds: Vec<String> = chosen
                .iter()
                .map(|(s, ..)| s.entry.meta.kind.as_str().to_owned())
                .collect();
            let sources: Vec<Option<String>> =
                chosen.iter().map(|(s, ..)| source_label(s.entry)).collect();
            let entries: Vec<InjectedEntry<'_>> = chosen
                .iter()
                .enumerate()
                .map(|(i, (s, text, truncated, _))| InjectedEntry {
                    id: &s.entry.meta.id,
                    version: s.entry.version(),
                    title: &s.entry.meta.title,
                    scope: s.entry.meta.scope,
                    kind: &kinds[i],
                    status: s.entry.meta.status,
                    authority: s.entry.meta.authority.as_str(),
                    source: sources[i].clone(),
                    text,
                    truncated: *truncated,
                })
                .collect();
            result.body = Some(reminder::format(Some(&project.name), workflow, &entries));
        }
        result.duration_ms = started.elapsed().as_millis() as u64;
        let audit = json!({
            "event": "retrieval", "at": now(), "session": session, "workflow": workflow.as_str(),
            "query_sha256": sha256_hex(prompt.as_bytes()), "status": result.status,
            "retrieved": scored.iter().take(20).map(|s| json!({"id": s.entry.meta.id, "scope": s.entry.meta.scope.as_str(), "score": (s.hit.score * 1000.0).round() / 1000.0})).collect::<Vec<_>>(),
            "injected": result.injected.iter().map(|i| json!({"id": i.id, "tokens": i.tokens})).collect::<Vec<_>>(),
            "filtered": filtered, "index_versions": result.index_versions, "duration_ms": result.duration_ms,
        });
        let audit_store = if snapshots[0].entries.is_empty() {
            &snapshots[1].store
        } else {
            &snapshots[0].store
        };
        let _ = audit_store.audit(audit).await;
        self.turn_cache.lock().await.put(key, result.clone());
        Ok(result)
    }

    pub fn scope_of(snapshots: &[Snapshot], id: &str) -> Option<(Scope, Entry)> {
        snapshots.iter().find_map(|s| {
            s.entries
                .get(id)
                .filter(|e| in_scope(e, s))
                .map(|e| (s.store.scope, e.clone()))
        })
    }
}
