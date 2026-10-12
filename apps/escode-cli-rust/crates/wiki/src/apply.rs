//! 把一个来源的候选知识写入存储（spec §6「增量」、§1 规则 3/6/7）：
//! - 新章节 → 新 proposal（或在用户显式 `approve` 时直接批准）；
//! - imported proposal → 原地更新；
//! - 已批准条目内容变化 → 不覆盖：标 stale，另建 `<id>.rev-<hash>` 更新提案（supersedes 原条目）；
//! - 消失的章节 → imported proposal 弃用，已批准条目标 stale；
//! - 已弃用/拒绝/替代的条目不会被导入「复活」。
use super::report::Item;
use super::store::ScopeStore;
use anyhow::Result;
use escode_cli_domain::wiki::entry::*;
use escode_cli_domain::wiki::extract::Candidate;
use escode_cli_domain::wiki::ir::DocumentIr;
use escode_cli_domain::wiki::text::short_hash;
use std::collections::BTreeMap;

pub struct Applied {
    pub changed: Vec<Entry>,
    pub sections: BTreeMap<String, String>,
}

pub struct Context<'a> {
    pub store: &'a ScopeStore,
    pub ir: &'a DocumentIr,
    pub now: &'a str,
    /// 用户在本次导入时显式批准（CLI `--approve` / WikiManage approve_imported 经确认）。
    pub approve: bool,
}

fn source_ref(ctx: &Context<'_>, candidate: &Candidate) -> SourceRef {
    SourceRef {
        document_id: ctx.ir.document_id.clone(),
        source_path: ctx.ir.source_path.clone(),
        content_hash: ctx.ir.content_hash.clone(),
        format: ctx.ir.source_format.clone(),
        locator: candidate.locator.clone(),
        excerpt: Some(candidate.excerpt.clone()),
        parser: ctx.ir.parser_name.clone(),
        parser_version: ctx.ir.parser_version.clone(),
        extracted_at: ctx.now.to_owned(),
    }
}

fn review(action: &str, now: &str, note: &str) -> ReviewEvent {
    ReviewEvent {
        action: action.into(),
        at: now.into(),
        by: "user".into(),
        note: Some(note.into()),
    }
}

pub fn new_entry(ctx: &Context<'_>, candidate: &Candidate, id: &str) -> Entry {
    let scope = ctx.store.scope;
    let (status, authority, reviews) = if ctx.approve {
        (
            Status::Approved,
            approved_authority(scope, candidate.kind),
            vec![review("approve", ctx.now, "approved at import")],
        )
    } else {
        (Status::Proposal, Authority::Proposal, vec![])
    };
    Entry {
        meta: Meta {
            schema_version: SCHEMA_VERSION,
            id: id.to_owned(),
            title: candidate.title.clone(),
            scope,
            project_id: ctx.store.project_id.clone(),
            kind: candidate.kind,
            status,
            authority,
            tags: candidate.tags.clone(),
            related: vec![],
            supersedes: None,
            superseded_by: None,
            applies_to: AppliesTo::default(),
            sources: vec![source_ref(ctx, candidate)],
            origin: Origin::Imported,
            confidence: ctx.ir.confidence,
            validity: ValidityInfo {
                state: Validity::Valid,
                reason: None,
            },
            created_at: ctx.now.to_owned(),
            updated_at: ctx.now.to_owned(),
            verified_at: ctx.approve.then(|| ctx.now.to_owned()),
            review: reviews,
            extra: Default::default(),
        },
        body: candidate.body.clone(),
    }
}

pub async fn apply(
    ctx: &Context<'_>,
    candidates: &[Candidate],
    previous: &BTreeMap<String, String>,
    item: &mut Item,
) -> Result<Applied> {
    let mut changed = vec![];
    let mut sections = BTreeMap::new();
    for candidate in candidates {
        let id = previous
            .get(&candidate.section_key)
            .cloned()
            .unwrap_or_else(|| candidate.id.clone());
        sections.insert(candidate.section_key.clone(), id.clone());
        let Some(mut existing) = ctx.store.read_entry(&id).await? else {
            let entry = new_entry(ctx, candidate, &id);
            ctx.store.write_entry(&entry).await?;
            item.created += 1;
            changed.push(entry);
            continue;
        };
        if existing.retired() {
            continue;
        }
        let same_body = existing.body.trim() == candidate.body.trim();
        let fresh_source = source_ref(ctx, candidate);
        if existing.meta.status == Status::Proposal && existing.meta.origin == Origin::Imported {
            let refreshed = new_entry(ctx, candidate, &id);
            if same_body && existing.meta.title == candidate.title && !ctx.approve {
                continue;
            }
            existing.body = refreshed.body;
            existing.meta.title = refreshed.meta.title;
            existing.meta.kind = refreshed.meta.kind;
            existing.meta.sources = refreshed.meta.sources;
            existing.meta.status = refreshed.meta.status;
            existing.meta.authority = refreshed.meta.authority;
            existing.meta.review.extend(refreshed.meta.review);
            existing.meta.updated_at = ctx.now.to_owned();
            ctx.store.write_entry(&existing).await?;
            item.updated += 1;
            changed.push(existing);
            continue;
        }
        if same_body {
            // 内容未变：只刷新来源哈希并清除因来源变化留下的 stale 标记。
            let was_stale = matches!(
                existing.meta.validity.state,
                Validity::Stale | Validity::BrokenSource
            );
            let hash_changed = existing
                .meta
                .sources
                .first()
                .is_none_or(|s| s.content_hash != fresh_source.content_hash);
            if was_stale || hash_changed {
                existing.meta.sources = vec![fresh_source];
                existing.meta.validity = ValidityInfo {
                    state: Validity::Valid,
                    reason: None,
                };
                existing.meta.updated_at = ctx.now.to_owned();
                ctx.store.write_entry(&existing).await?;
                changed.push(existing);
            }
            continue;
        }
        if ctx.approve {
            // 用户显式批准本次导入：原地更新已批准条目并记录审核事件。
            existing.body = candidate.body.clone();
            existing.meta.title = candidate.title.clone();
            existing.meta.sources = vec![fresh_source];
            existing.meta.validity = ValidityInfo {
                state: Validity::Valid,
                reason: None,
            };
            existing.meta.review.push(review(
                "approve-update",
                ctx.now,
                "source update approved at import",
            ));
            existing.meta.updated_at = ctx.now.to_owned();
            existing.meta.verified_at = Some(ctx.now.to_owned());
            ctx.store.write_entry(&existing).await?;
            item.updated += 1;
            changed.push(existing);
            continue;
        }
        // 已批准知识不被未审核导入静默覆盖：标 stale + 更新提案。
        let revision_id = format!("{id}.rev-{}", short_hash(&candidate.body, 6));
        if ctx.store.read_entry(&revision_id).await?.is_none() {
            let mut proposal = new_entry(ctx, candidate, &revision_id);
            proposal.meta.supersedes = Some(id.clone());
            proposal.meta.related = vec![id.clone()];
            ctx.store.write_entry(&proposal).await?;
            item.proposals += 1;
            changed.push(proposal);
        }
        if existing.meta.validity.state != Validity::Stale {
            existing.meta.validity = ValidityInfo {
                state: Validity::Stale,
                reason: Some(format!(
                    "source {} changed at {}; review update proposal {revision_id}",
                    ctx.ir.source_path, candidate.locator.value
                )),
            };
            existing.meta.updated_at = ctx.now.to_owned();
            ctx.store.write_entry(&existing).await?;
            item.staled += 1;
            changed.push(existing);
        }
    }
    for (key, id) in previous {
        if sections.contains_key(key) {
            continue;
        }
        let Some(mut existing) = ctx.store.read_entry(id).await? else {
            continue;
        };
        if existing.retired() {
            continue;
        }
        if existing.meta.status == Status::Proposal && existing.meta.origin == Origin::Imported {
            existing.meta.status = Status::Deprecated;
            existing.meta.review.push(ReviewEvent {
                action: "deprecate".into(),
                at: ctx.now.into(),
                by: "import".into(),
                note: Some("section no longer present in the source".into()),
            });
            item.retired += 1;
        } else {
            existing.meta.validity = ValidityInfo {
                state: Validity::Stale,
                reason: Some(format!(
                    "section '{key}' no longer present in {}",
                    ctx.ir.source_path
                )),
            };
            item.staled += 1;
        }
        existing.meta.updated_at = ctx.now.to_owned();
        ctx.store.write_entry(&existing).await?;
        changed.push(existing);
    }
    Ok(Applied { changed, sections })
}
