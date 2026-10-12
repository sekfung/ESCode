//! 治理写操作（spec §6.5、§12）：提案、批准、拒绝、弃用/替代、提升作用域、重建索引。
//! 全部在 scope 写锁内执行并写审计；批准只能由用户显式触发（CLI / Desktop / alwaysAsk 工具）。
use super::fsio::ScopeLock;
use super::service::{WikiService, now};
use super::store::ScopeStore;
use anyhow::{Result, bail, ensure};
use escode_cli_domain::wiki::entry::*;
use escode_cli_domain::wiki::index::Index;
use escode_cli_domain::wiki::text::{short_hash, slug, tokenize};
use escode_cli_domain::wiki::tools::ProposeInput;
use serde_json::json;
use std::collections::BTreeSet;

/// 明显的密钥/凭据形态：提案里出现即拒绝（防止敏感信息沉淀到 Wiki，尤其是 Global Wiki）。
fn looks_secret(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("-----begin") && lower.contains("private key") {
        return true;
    }
    let akia = text.as_bytes().windows(20).any(|w| {
        w.starts_with(b"AKIA")
            && w[4..]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    });
    let assignment = [
        "api_key",
        "apikey",
        "api-key",
        "secret",
        "password",
        "passwd",
        "access_token",
        "auth_token",
    ]
    .iter()
    .any(|key| {
        lower.match_indices(key).any(|(i, _)| {
            let rest = lower[i + key.len()..].trim_start();
            let value = rest
                .strip_prefix(['=', ':'])
                .map(str::trim_start)
                .unwrap_or("");
            let value: String = value
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ',')
                .collect();
            value.trim_matches(['"', '\'']).len() >= 12
        })
    });
    akia || assignment
}

/// 提案正文去掉追加的 Rationale / Evidence 段，只比较规则本身。
fn core_body(body: &str) -> &str {
    body.split("\n\n## Rationale").next().unwrap_or(body)
}

fn similarity(a: &str, b: &str) -> f64 {
    let a: BTreeSet<String> = tokenize(a).into_iter().collect();
    let b: BTreeSet<String> = tokenize(b).into_iter().collect();
    let union = a.union(&b).count();
    if union == 0 {
        0.0
    } else {
        a.intersection(&b).count() as f64 / union as f64
    }
}

pub(crate) async fn write_and_index(store: &ScopeStore, entries: &[Entry]) -> Result<()> {
    for entry in entries {
        store.write_entry(entry).await?;
    }
    let loaded = store.load().await?;
    let (mut index, _) = store.index(&loaded.entries, true).await?;
    for entry in entries {
        index.upsert(entry);
    }
    store.write_index(&index).await
}

impl WikiService {
    pub async fn propose(
        &self,
        input: &ProposeInput,
        session: &str,
    ) -> Result<(Entry, Vec<String>)> {
        let settings = self.settings().await;
        ensure!(settings.config.enabled, "wiki is disabled");
        let kind = Kind::parse(&input.kind)
            .filter(|k| *k != Kind::Proposal)
            .ok_or_else(|| anyhow::anyhow!("invalid kind '{}'", input.kind))?;
        let scope = match input.scope.as_deref() {
            None | Some("project") => Scope::Project,
            Some("global") => Scope::Global,
            Some(other) => bail!("invalid scope '{other}'"),
        };
        ensure!(
            (3..=200).contains(&input.title.trim().chars().count()),
            "title must be 3-200 characters"
        );
        ensure!(
            !input.body.trim().is_empty() && input.body.chars().count() <= 20_000,
            "body must be 1-20000 characters"
        );
        let full = format!("{}\n{}\n{}", input.title, input.body, input.rationale);
        ensure!(
            !looks_secret(&full),
            "proposal appears to contain a secret or credential; remove it before proposing"
        );
        let mut body = input.body.trim().to_owned();
        body.push_str(&format!("\n\n## Rationale\n\n{}", input.rationale.trim()));
        if let Some(evidence) = input.evidence.as_ref().filter(|e| !e.is_empty()) {
            body.push_str("\n\n## Evidence\n");
            for e in evidence {
                let location = match (&e.file, e.line) {
                    (Some(f), Some(l)) => format!("`{f}:{l}` — "),
                    (Some(f), None) => format!("`{f}` — "),
                    _ => String::new(),
                };
                body.push_str(&format!("\n- {location}{}", e.note.trim()));
            }
        }
        let store = self.store_for(&settings, scope).await;
        let snapshots = self.open_snapshots().await?;
        let duplicates: Vec<String> = snapshots
            .iter()
            .flat_map(|s| s.entries.values())
            .filter(|e| {
                !e.retired()
                    && similarity(
                        &format!("{} {}", e.meta.title, core_body(&e.body)),
                        &format!("{} {}", input.title, input.body),
                    ) >= 0.8
            })
            .map(|e| e.meta.id.clone())
            .collect();
        let id = format!(
            "prop-{}-{}",
            slug(&input.title, 48),
            short_hash(&format!("{}\n{}", input.title, input.body), 6)
        );
        let _lock = ScopeLock::acquire(&store.root).await?;
        if let Some(existing) = store.read_entry(&id).await? {
            return Ok((existing, duplicates));
        }
        let stamp = now();
        let entry = Entry {
            meta: Meta {
                schema_version: SCHEMA_VERSION,
                id: id.clone(),
                title: input.title.trim().to_owned(),
                scope,
                project_id: store.project_id.clone(),
                kind,
                status: Status::Proposal,
                authority: Authority::Proposal,
                tags: input.tags.clone().unwrap_or_default(),
                related: duplicates.clone(),
                supersedes: None,
                superseded_by: None,
                applies_to: AppliesTo {
                    paths: input.applies_to_paths.clone().unwrap_or_default(),
                    versions: None,
                },
                sources: vec![],
                origin: Origin::AgentProposal,
                confidence: None,
                validity: ValidityInfo {
                    state: Validity::Unverified,
                    reason: None,
                },
                created_at: stamp.clone(),
                updated_at: stamp.clone(),
                verified_at: None,
                review: vec![ReviewEvent {
                    action: "propose".into(),
                    at: stamp.clone(),
                    by: format!("agent:{session}"),
                    note: None,
                }],
                extra: Default::default(),
            },
            body,
        };
        write_and_index(&store, std::slice::from_ref(&entry)).await?;
        store.audit(json!({"event": "propose", "at": stamp, "id": id, "scope": scope.as_str(), "session": session})).await?;
        self.invalidate().await;
        Ok((entry, duplicates))
    }

    /// 在 project → global 顺序中查找条目所在存储。
    pub async fn locate(&self, id: &str) -> Result<(ScopeStore, Entry)> {
        let settings = self.settings().await;
        let (project, global) = self.stores(&settings).await;
        for store in [project, global] {
            if let Some(entry) = store.read_entry(id).await? {
                return Ok((store, entry));
            }
        }
        bail!("wiki entry '{id}' not found in the global wiki or this project's wiki")
    }

    /// 状态变更：approve / reject / deprecate。`by` 记录发起方（cli / desktop / agent-approved）。
    pub async fn review(
        &self,
        action: &str,
        ids: &[String],
        reason: Option<&str>,
        superseded_by: Option<&str>,
        by: &str,
    ) -> Result<Vec<(String, String)>> {
        ensure!(!ids.is_empty(), "no entry ids given");
        let mut results = vec![];
        for id in ids {
            let outcome = self.review_one(action, id, reason, superseded_by, by).await;
            results.push((
                id.clone(),
                match outcome {
                    Ok(message) => message,
                    Err(error) => format!("error: {error:#}"),
                },
            ));
        }
        self.invalidate().await;
        Ok(results)
    }

    async fn review_one(
        &self,
        action: &str,
        id: &str,
        reason: Option<&str>,
        superseded_by: Option<&str>,
        by: &str,
    ) -> Result<String> {
        let (store, mut entry) = self.locate(id).await?;
        let _lock = ScopeLock::acquire(&store.root).await?;
        let stamp = now();
        let mut touched = vec![];
        let event = |action: &str| ReviewEvent {
            action: action.into(),
            at: stamp.clone(),
            by: by.into(),
            note: reason.map(str::to_owned),
        };
        match action {
            "approve" => {
                if entry.meta.status == Status::Approved
                    && entry.meta.validity.state == Validity::Valid
                {
                    return Ok("already approved".into());
                }
                ensure!(
                    matches!(
                        entry.meta.status,
                        Status::Proposal | Status::NeedsReview | Status::Approved
                    ),
                    "cannot approve a {} entry",
                    entry.meta.status.as_str()
                );
                ensure!(
                    entry.meta.kind != Kind::Proposal,
                    "set a concrete kind before approving"
                );
                entry.meta.status = Status::Approved;
                entry.meta.authority = approved_authority(entry.meta.scope, entry.meta.kind);
                if entry.meta.validity.state != Validity::BrokenSource {
                    entry.meta.validity = ValidityInfo {
                        state: Validity::Valid,
                        reason: None,
                    };
                }
                entry.meta.verified_at = Some(stamp.clone());
                entry.meta.review.push(event("approve"));
                // 批准更新提案：被替代的旧条目转为 superseded，保留历史与关系。
                if let Some(old_id) = entry.meta.supersedes.clone()
                    && let Some(mut old) = store.read_entry(&old_id).await?
                    && !old.retired()
                {
                    old.meta.status = Status::Superseded;
                    old.meta.superseded_by = Some(entry.meta.id.clone());
                    old.meta.updated_at = stamp.clone();
                    old.meta.review.push(event("superseded"));
                    touched.push(old);
                }
            }
            "reject" => {
                ensure!(
                    entry.meta.status == Status::Proposal,
                    "only proposals can be rejected (use deprecate for approved entries)"
                );
                entry.meta.status = Status::Rejected;
                entry.meta.review.push(event("reject"));
            }
            "deprecate" => {
                ensure!(
                    !entry.retired(),
                    "entry is already {}",
                    entry.meta.status.as_str()
                );
                if let Some(target) = superseded_by {
                    self.locate(target).await?;
                    entry.meta.status = Status::Superseded;
                    entry.meta.superseded_by = Some(target.to_owned());
                } else {
                    entry.meta.status = Status::Deprecated;
                }
                entry.meta.review.push(event("deprecate"));
            }
            other => bail!("unknown review action '{other}'"),
        }
        entry.meta.updated_at = stamp.clone();
        touched.insert(0, entry);
        write_and_index(&store, &touched).await?;
        store.audit(json!({"event": action, "at": stamp, "id": id, "scope": store.scope.as_str(), "by": by, "reason": reason})).await?;
        Ok(touched[0].meta.status.as_str().to_owned())
    }

    /// 把项目条目复制到 Global Wiki，作为新的提案（扩大适用范围需要再次批准）。
    pub async fn promote(&self, id: &str, by: &str) -> Result<String> {
        let (store, entry) = self.locate(id).await?;
        ensure!(
            store.scope == Scope::Project,
            "only project entries can be promoted"
        );
        ensure!(
            !looks_secret(&entry.body),
            "entry appears to contain a secret; it cannot be promoted to the global wiki"
        );
        let settings = self.settings().await;
        let global = ScopeStore::global(&settings.root);
        let _lock = ScopeLock::acquire(&global.root).await?;
        let new_id = format!(
            "{}.global",
            entry.meta.id.chars().take(120).collect::<String>()
        );
        let stamp = now();
        let mut copy = entry.clone();
        copy.meta.id = new_id.clone();
        copy.meta.scope = Scope::Global;
        copy.meta.project_id = None;
        copy.meta.status = Status::Proposal;
        copy.meta.authority = Authority::Proposal;
        copy.meta.related = vec![];
        copy.meta.created_at = stamp.clone();
        copy.meta.updated_at = stamp.clone();
        copy.meta.review = vec![ReviewEvent {
            action: "promote".into(),
            at: stamp.clone(),
            by: by.into(),
            note: Some(format!("copied from project entry {id}")),
        }];
        write_and_index(&global, std::slice::from_ref(&copy)).await?;
        global
            .audit(
                json!({"event": "scope_change", "at": stamp, "from": id, "to": new_id, "by": by}),
            )
            .await?;
        self.invalidate().await;
        Ok(new_id)
    }

    pub async fn rebuild_indexes(&self) -> Result<Vec<(String, u64, usize)>> {
        let settings = self.settings().await;
        let (project, global) = self.stores(&settings).await;
        let mut out = vec![];
        for store in [project, global] {
            let _lock = ScopeLock::acquire(&store.root).await?;
            let loaded = store.load().await?;
            let mut index = Index::build(loaded.entries.iter());
            index.version = store.index_version().await + 1;
            store.write_index(&index).await?;
            store.audit(json!({"event": "rebuild_index", "at": now(), "version": index.version, "entries": loaded.entries.len()})).await?;
            out.push((
                store.scope.as_str().to_owned(),
                index.version,
                loaded.entries.len(),
            ));
        }
        self.invalidate().await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn secret_screening() {
        assert!(super::looks_secret("api_key = sk_live_abcdefghijklmnop"));
        assert!(super::looks_secret("AKIAABCDEFGHIJKLMNOP"));
        assert!(!super::looks_secret(
            "Never log the api_key value; read it from the vault."
        ));
    }
}
