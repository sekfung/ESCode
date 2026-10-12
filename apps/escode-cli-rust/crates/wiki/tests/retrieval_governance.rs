//! 验收 §15.2（适配器侧）与 §15.5：检索注入规则、审计、治理流程。
mod common;
use common::*;
use escode_cli_domain::wiki::entry::{Scope, Status};
use escode_cli_domain::wiki::tools::ProposeInput;
use escode_cli_wiki::WikiService;
use std::collections::HashMap;

fn input(title: &str, body: &str) -> ProposeInput {
    ProposeInput {
        title: title.into(),
        body: body.into(),
        kind: "standard".into(),
        rationale: "agreed in review".into(),
        scope: None,
        tags: None,
        applies_to_paths: None,
        evidence: None,
    }
}

async fn approved(wiki: &WikiService, title: &str, body: &str) -> String {
    let (e, _) = wiki.propose(&input(title, body), "s").await.unwrap();
    wiki.review(
        "approve",
        std::slice::from_ref(&e.meta.id),
        None,
        None,
        "test",
    )
    .await
    .unwrap();
    e.meta.id
}

fn setup() -> (Env, std::path::PathBuf) {
    let env = Env::new();
    let repo = env.repo("app", Some("https://h/o/app"));
    (env, repo)
}

#[tokio::test]
async fn injection_rules_budget_dedupe_and_audit() {
    let (env, repo) = setup();
    let wiki = env.service(&repo);
    let errors = approved(
        &wiki,
        "API error envelope",
        "Every HTTP handler must return errors in the problem+json envelope with a stable code.",
    )
    .await;
    let (pending, _) = wiki
        .propose(
            &input(
                "Error retries proposal",
                "Handlers should retry errors three times before returning an error envelope.",
            ),
            "s",
        )
        .await
        .unwrap();
    let _unrelated = approved(
        &wiki,
        "Frontend color tokens",
        "Use semantic color tokens in the design system for buttons.",
    )
    .await;
    let result = wiki
        .retrieve(
            "s1",
            "implement the error handling for the orders HTTP handler",
            &HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.status, "injected");
    assert_eq!(result.workflow, "coding");
    let ids: Vec<&str> = result.injected.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![errors.as_str()],
        "proposal and unrelated entries are not injected"
    );
    assert!(result.retrieved >= 2 && result.filtered >= 1, "{result:?}");
    let body = result.body.unwrap();
    assert!(body.contains(&format!("id=\"{errors}\"")) && !body.contains(&pending.meta.id));
    assert!(body.contains("not instructions") && !body.contains("Review protocol"));
    // 已注入的同版本条目不重复注入。
    let already = HashMap::from([(errors.clone(), result.injected[0].version.clone())]);
    let second = wiki
        .retrieve(
            "s1",
            "implement the error handling for the orders HTTP handler",
            &already,
        )
        .await
        .unwrap();
    assert_eq!(second.status, "no_match");
    // 审计区分「检索到」与「注入」，且不记录原始问题文本。
    let audit = wiki.audit_tail(10).await.unwrap();
    let event = audit
        .iter()
        .rev()
        .find(|e| e["event"] == "retrieval" && e["status"] == "injected")
        .unwrap();
    assert!(
        event["retrieved"].as_array().unwrap().len() > event["injected"].as_array().unwrap().len()
    );
    assert!(!event.to_string().contains("orders HTTP handler"));
    assert!(event["query_sha256"].as_str().unwrap().len() == 64);
    // 预算：maxInjectedTokens 很小时不注入超预算条目（宁可不注入，也不截断成半条规范）。
    env.user_config(serde_json::json!({"wiki": {"retrieval": {"maxInjectedTokens": 10}}}));
    let tight = wiki
        .retrieve(
            "s2",
            "implement the error handling for the orders HTTP handler",
            &HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(tight.status, "no_match");
    // 关闭自动注入 / 关闭 Wiki。
    env.user_config(serde_json::json!({"wiki": {"autoInject": false}}));
    assert_eq!(
        wiki.retrieve("s3", "error handler", &HashMap::new())
            .await
            .unwrap()
            .status,
        "disabled"
    );
    env.user_config(serde_json::json!({"wiki": {"enabled": false}}));
    assert_eq!(
        wiki.retrieve("s3", "error handler", &HashMap::new())
            .await
            .unwrap()
            .status,
        "disabled"
    );
    assert!(
        wiki.tool("WikiSearch", &serde_json::json!({"query": "error"}), "s")
            .await
            .unwrap()
            .failed
    );
}

#[tokio::test]
async fn review_workflow_adds_protocol_and_changed_files() {
    let (env, repo) = setup();
    let wiki = env.service(&repo);
    let (e, _) = wiki
        .propose(
            &ProposeInput {
                applies_to_paths: Some(vec!["src/payments/**".into()]),
                ..input(
                    "Payment idempotency",
                    "Payment handlers must check the idempotency key before charging.",
                )
            },
            "s",
        )
        .await
        .unwrap();
    wiki.review(
        "approve",
        std::slice::from_ref(&e.meta.id),
        None,
        None,
        "test",
    )
    .await
    .unwrap();
    let result = wiki
        .retrieve(
            "s",
            "please review the change in src/payments/charge.rs",
            &HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.workflow, "review");
    let body = result.body.unwrap();
    assert!(body.contains("Review protocol") && body.contains("needs-confirmation"));
    assert!(body.contains(&e.meta.id));
    // 引用核对：回复里引用的 id 必须真实存在。
    let cited = escode_cli_domain::wiki::reminder::cited_ids(&format!(
        "violates [wiki:{}] and [wiki:made-up]",
        e.meta.id
    ));
    let detail_ok = wiki.entry_detail(&cited[0]).await.unwrap();
    let detail_bad = wiki.entry_detail(&cited[1]).await.unwrap();
    assert_eq!(
        (detail_ok["found"].as_bool(), detail_bad["found"].as_bool()),
        (Some(true), Some(false))
    );
}

#[tokio::test]
async fn governance_flows() {
    let (env, repo) = setup();
    let wiki = env.service(&repo);
    // 敏感信息不能进入提案。
    let secret = wiki
        .propose(
            &input("DB access", "Connect with password = hunter2hunter2hunter2"),
            "s",
        )
        .await;
    assert!(secret.unwrap_err().to_string().contains("secret"));
    let (p, dup) = wiki
        .propose(
            &input(
                "Cache invalidation",
                "Caches must be invalidated on every write to the orders table.",
            ),
            "s",
        )
        .await
        .unwrap();
    assert!(dup.is_empty());
    let (_, dup2) = wiki
        .propose(
            &input(
                "Cache invalidation rule",
                "Caches must be invalidated on every write to the orders table.",
            ),
            "s",
        )
        .await
        .unwrap();
    assert_eq!(dup2, vec![p.meta.id.clone()], "near-duplicate detected");
    // 候选不会自动升级；拒绝后不能再注入。
    assert_eq!(
        wiki.retrieve("s", "invalidate caches on write to orders", &HashMap::new())
            .await
            .unwrap()
            .status,
        "no_match"
    );
    let out = wiki
        .review(
            "reject",
            std::slice::from_ref(&p.meta.id),
            Some("duplicate"),
            None,
            "cli",
        )
        .await
        .unwrap();
    assert_eq!(out[0].1, "rejected");
    let bad = wiki
        .review(
            "approve",
            std::slice::from_ref(&p.meta.id),
            None,
            None,
            "cli",
        )
        .await
        .unwrap();
    assert!(
        bad[0].1.starts_with("error"),
        "rejected entries cannot be approved: {bad:?}"
    );
    // 弃用并指定替代；提升到 global 需要再次批准。
    let keep = approved(
        &wiki,
        "Orders cache rule",
        "Orders caches must be invalidated on every write.",
    )
    .await;
    let replacement = approved(
        &wiki,
        "Orders cache rule v2",
        "Orders caches must be invalidated on write and on schema migration.",
    )
    .await;
    let dep = wiki
        .review(
            "deprecate",
            std::slice::from_ref(&keep),
            Some("replaced"),
            Some(&replacement),
            "cli",
        )
        .await
        .unwrap();
    assert_eq!(dep[0].1, "superseded");
    let new_id = wiki.promote(&replacement, "cli").await.unwrap();
    let (store, promoted) = wiki.locate(&new_id).await.unwrap();
    assert_eq!(
        (store.scope, promoted.meta.status),
        (Scope::Global, Status::Proposal)
    );
    // lint 与审计记录。
    let lint = wiki.lint().await.unwrap();
    assert!(lint.iter().all(|i| i.severity != "error"), "{lint:?}");
    let audit = wiki.audit_tail(50).await.unwrap();
    for event in ["propose", "approve", "reject", "deprecate", "scope_change"] {
        assert!(
            audit.iter().any(|e| e["event"] == event),
            "missing audit {event}"
        );
    }
}
