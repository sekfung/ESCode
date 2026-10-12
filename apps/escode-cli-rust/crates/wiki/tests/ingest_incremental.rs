//! 验收 §15.4：来源追溯、幂等、增量、删除/移动、失败恢复、索引重建、并发。
mod common;
use common::*;
use escode_cli_domain::wiki::entry::{Scope, Status, Validity};
use std::path::PathBuf;

const GUIDE: &str = "# Engineering Guide\n\n## API Error Handling\n\nHandlers must return the standard error envelope with a code field.\n\n## Deployment\n\nRun the build step and the smoke tests before every deploy.\n";

async fn entries(
    service: &escode_cli_wiki::WikiService,
) -> Vec<escode_cli_domain::wiki::entry::Entry> {
    let settings = service.settings().await;
    let store = service.store_for(&settings, Scope::Project).await;
    let mut all = store.load().await.unwrap().entries;
    all.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    all
}

#[tokio::test]
async fn ingest_is_traceable_idempotent_and_incremental() {
    let env = Env::new();
    let repo = env.repo("svc", Some("https://h/o/svc"));
    let doc = write(&repo.join("docs/guide.md"), GUIDE);
    let wiki = env.service(&repo);
    let first = wiki
        .ingest(std::slice::from_ref(&doc), Scope::Project, false)
        .await
        .unwrap();
    assert_eq!(first.items[0].status, "imported", "{first:?}");
    assert_eq!(first.items[0].created, 2);
    let created = entries(&wiki).await;
    assert!(
        created.iter().all(|e| e.meta.status == Status::Proposal),
        "import is not approval"
    );
    let api = created
        .iter()
        .find(|e| e.meta.title.contains("API Error Handling"))
        .unwrap()
        .clone();
    let source = &api.meta.sources[0];
    assert_eq!(source.source_path, "docs/guide.md");
    assert!(
        source
            .locator
            .value
            .starts_with("Engineering Guide / API Error Handling (lines 3-"),
        "{:?}",
        source.locator
    );
    assert!(
        source.content_hash.starts_with("sha256:")
            && source
                .excerpt
                .as_deref()
                .unwrap()
                .contains("standard error envelope")
    );

    // 重复导入：unchanged，不产生新条目。
    let again = wiki
        .ingest(std::slice::from_ref(&doc), Scope::Project, false)
        .await
        .unwrap();
    assert_eq!(again.items[0].status, "unchanged");
    assert_eq!(entries(&wiki).await.len(), 2);

    // 批准一条，再修改两个章节中的一个：已批准条目不被覆盖（stale + 更新提案），未批准的原地更新。
    wiki.review(
        "approve",
        std::slice::from_ref(&api.meta.id),
        None,
        None,
        "test",
    )
    .await
    .unwrap();
    write(
        &doc,
        &GUIDE
            .replace("with a code field", "with code and trace_id fields")
            .replace("smoke tests", "integration tests"),
    );
    let changed = wiki
        .ingest(std::slice::from_ref(&doc), Scope::Project, false)
        .await
        .unwrap();
    let item = &changed.items[0];
    assert_eq!(
        (item.status, item.updated, item.staled, item.proposals),
        ("updated", 1, 1, 1),
        "{item:?}"
    );
    let now = entries(&wiki).await;
    let approved = now.iter().find(|e| e.meta.id == api.meta.id).unwrap();
    assert_eq!(approved.meta.status, Status::Approved);
    assert_eq!(approved.body, api.body, "approved body untouched");
    assert_eq!(approved.meta.validity.state, Validity::Stale);
    let revision = now
        .iter()
        .find(|e| e.meta.supersedes.as_deref() == Some(api.meta.id.as_str()))
        .unwrap();
    assert!(revision.body.contains("trace_id") && revision.meta.status == Status::Proposal);
    let conflicts = wiki.conflicts().await.unwrap();
    assert!(conflicts.iter().any(|c| c.kind == "pending_update"));
    // 批准更新提案 → 旧条目 superseded，关系保留。
    wiki.review(
        "approve",
        std::slice::from_ref(&revision.meta.id),
        None,
        None,
        "test",
    )
    .await
    .unwrap();
    let after = entries(&wiki).await;
    let old = after.iter().find(|e| e.meta.id == api.meta.id).unwrap();
    assert_eq!(old.meta.status, Status::Superseded);
    assert_eq!(
        old.meta.superseded_by.as_deref(),
        Some(revision.meta.id.as_str())
    );
}

#[tokio::test]
async fn deletion_and_move_never_delete_knowledge() {
    let env = Env::new();
    let repo = env.repo("svc", Some("https://h/o/svc2"));
    let doc = write(&repo.join("docs/guide.md"), GUIDE);
    let wiki = env.service(&repo);
    wiki.ingest(std::slice::from_ref(&doc), Scope::Project, true)
        .await
        .unwrap();
    let before = entries(&wiki).await;
    assert!(
        before.iter().all(|e| e.meta.status == Status::Approved),
        "--approve is an explicit user approval"
    );
    // 移动：同内容出现在新路径，document_id 保持，来源路径更新。
    let moved = repo.join("handbook/guide-v2.md");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&doc, &moved).unwrap();
    let report = wiki.sync(Scope::Project).await.unwrap();
    assert_eq!(report.items[0].status, "moved", "{report:?}");
    let after_move = entries(&wiki).await;
    assert_eq!(after_move.len(), before.len());
    assert!(
        after_move
            .iter()
            .all(|e| e.meta.sources[0].source_path == "handbook/guide-v2.md")
    );
    // 删除：条目保留，标记 broken-source。
    std::fs::remove_file(&moved).unwrap();
    let report = wiki.sync(Scope::Project).await.unwrap();
    assert_eq!(report.items[0].status, "missing");
    let after_delete = entries(&wiki).await;
    assert_eq!(after_delete.len(), before.len());
    assert!(
        after_delete
            .iter()
            .all(|e| e.meta.validity.state == Validity::BrokenSource
                && e.meta.status == Status::Approved)
    );
    let drift = wiki.validate_sources().await.unwrap();
    assert_eq!(drift[0]["state"], "missing");
}

#[tokio::test]
async fn parse_failure_keeps_existing_knowledge_and_retry_recovers() {
    let env = Env::new();
    let repo = env.repo("svc", Some("https://h/o/svc3"));
    let config = write(
        &repo.join("config/limits.json"),
        r#"{"services": {"auth": {"timeout": 30}}}"#,
    );
    let wiki = env.service(&repo);
    assert_eq!(
        wiki.ingest(std::slice::from_ref(&config), Scope::Project, false)
            .await
            .unwrap()
            .items[0]
            .status,
        "imported"
    );
    let before = entries(&wiki).await;
    write(&config, "{\"services\": {\"auth\": ");
    let failed = wiki
        .ingest(std::slice::from_ref(&config), Scope::Project, false)
        .await
        .unwrap();
    assert_eq!(failed.items[0].status, "failed");
    assert!(
        failed.items[0].error.as_deref().unwrap().contains("line 1"),
        "{failed:?}"
    );
    assert_eq!(
        entries(&wiki).await,
        before,
        "failed parse must not touch existing entries"
    );
    write(&config, r#"{"services": {"auth": {"timeout": 45}}}"#);
    assert_eq!(
        wiki.ingest(&[config], Scope::Project, false)
            .await
            .unwrap()
            .items[0]
            .status,
        "updated"
    );
    assert!(entries(&wiki).await[0].body.contains("45"));
}

#[tokio::test]
async fn index_is_rebuildable_and_self_heals() {
    let env = Env::new();
    let repo = env.repo("svc", Some("https://h/o/svc4"));
    let doc = write(&repo.join("guide.md"), GUIDE);
    let wiki = env.service(&repo);
    wiki.ingest(&[doc], Scope::Project, true).await.unwrap();
    let settings = wiki.settings().await;
    let store = wiki.store_for(&settings, Scope::Project).await;
    let index_path = store.root.join("index.json");
    // 索引被删除或损坏：检索仍正确（从条目重建）。
    std::fs::write(&index_path, "{broken").unwrap();
    let hits = wiki
        .tool(
            "WikiSearch",
            &serde_json::json!({"query": "deploy smoke tests"}),
            "s",
        )
        .await
        .unwrap();
    assert!(hits.content.contains("Deployment"), "{}", hits.content);
    std::fs::remove_file(&index_path).unwrap();
    let rebuilt = wiki.rebuild_indexes().await.unwrap();
    assert_eq!(rebuilt[0].2, 2);
    assert!(index_path.exists());
    // 手工新增条目文件（绕过服务）→ 指纹不符 → 下次读取重建。
    let mut extra = store.load().await.unwrap().entries[0].clone();
    extra.meta.id = "manual-entry".into();
    extra.meta.title = "Manual caching rule".into();
    extra.body = "Caches must expire within ten minutes.".into();
    store.write_entry(&extra).await.unwrap();
    wiki.invalidate().await;
    let found = wiki
        .tool(
            "WikiSearch",
            &serde_json::json!({"query": "cache expire minutes"}),
            "s",
        )
        .await
        .unwrap();
    assert!(found.content.contains("manual-entry"), "{}", found.content);
}

#[tokio::test]
async fn concurrent_ingests_are_serialized_without_loss() {
    let env = Env::new();
    let repo = env.repo("svc", Some("https://h/o/svc5"));
    let files: Vec<PathBuf> = (0..6).map(|i| write(&repo.join(format!("docs/rule{i}.md")), &format!("# Rule {i}\n\n## Section {i}\n\nServices must follow rule number {i} carefully.\n"))).collect();
    let wiki = std::sync::Arc::new(env.service(&repo));
    let mut tasks = vec![];
    for file in files {
        let wiki = wiki.clone();
        tasks.push(tokio::spawn(async move {
            wiki.ingest(&[file], Scope::Project, false).await
        }));
    }
    for task in tasks {
        assert!(task.await.unwrap().unwrap().ok());
    }
    let all = entries(&wiki).await;
    assert_eq!(all.len(), 6);
    let settings = wiki.settings().await;
    let store = wiki.store_for(&settings, Scope::Project).await;
    let (_, rebuilt) = store.index(&all, false).await.unwrap();
    assert!(
        !rebuilt,
        "index fingerprint matches entries after concurrent writes"
    );
    assert!(!store.root.join(".lock").exists(), "lock released");
}
