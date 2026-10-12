//! 验收 §15.1：作用域隔离与项目身份。
mod common;
use common::*;
use escode_cli_domain::wiki::entry::Scope;
use escode_cli_domain::wiki::project::IdentityKind;
use escode_cli_domain::wiki::tools::ProposeInput;
use escode_cli_wiki::identity::resolve_project;
use std::collections::HashMap;

fn propose(title: &str, body: &str, scope: &str) -> ProposeInput {
    ProposeInput {
        title: title.into(),
        body: body.into(),
        kind: "standard".into(),
        rationale: "team decision".into(),
        scope: Some(scope.into()),
        tags: None,
        applies_to_paths: None,
        evidence: None,
    }
}

async fn approve_all(service: &escode_cli_wiki::WikiService, ids: &[String]) {
    service
        .review("approve", ids, None, None, "test")
        .await
        .unwrap();
}

#[tokio::test]
async fn global_knowledge_is_shared_and_project_knowledge_is_isolated() {
    let env = Env::new();
    let a = env.repo("alpha", Some("git@github.com:org/alpha.git"));
    let b = env.repo("beta", Some("git@github.com:org/beta.git"));
    let wa = env.service(&a);
    let wb = env.service(&b);
    let (global, _) = wa
        .propose(
            &propose(
                "Logging standard",
                "Services must use structured logging with trace ids.",
                "global",
            ),
            "s1",
        )
        .await
        .unwrap();
    let (secret, _) = wa
        .propose(
            &propose(
                "Alpha billing invariant",
                "Alpha invoices must never be deleted once issued.",
                "project",
            ),
            "s1",
        )
        .await
        .unwrap();
    approve_all(&wa, &[global.meta.id.clone(), secret.meta.id.clone()]).await;

    // 两个项目都能检索到同一条全局知识。
    for service in [&wa, &wb] {
        let hit = service
            .retrieve(
                "s",
                "add structured logging to the service",
                &HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(hit.status, "injected", "{hit:?}");
        assert!(hit.injected.iter().any(|i| i.id == global.meta.id));
    }
    // 项目 A 的知识在项目 B 的检索、读取、图谱中都不可见——即使提到项目 A 的名字。
    let probe = wb
        .retrieve(
            "s",
            "alpha billing invoices deleted invariant",
            &HashMap::new(),
        )
        .await
        .unwrap();
    assert!(
        probe.injected.iter().all(|i| i.id != secret.meta.id),
        "{probe:?}"
    );
    let read = wb
        .tool("WikiRead", &serde_json::json!({"id": secret.meta.id}), "s")
        .await
        .unwrap();
    assert!(
        read.failed,
        "cross-project read must fail: {}",
        read.content
    );
    let search = wb
        .tool(
            "WikiSearch",
            &serde_json::json!({"query": "alpha invoices"}),
            "s",
        )
        .await
        .unwrap();
    assert!(!search.content.contains(&secret.meta.id));
    let graph = wb.graph(&serde_json::json!({})).await.unwrap();
    assert!(!graph.to_string().contains(&secret.meta.id));
    // 工具入参不接受任何项目参数。
    let hack = wb
        .tool(
            "WikiSearch",
            &serde_json::json!({"query": "x", "project": "alpha"}),
            "s",
        )
        .await
        .unwrap();
    assert!(hack.failed);
    // 项目 A 仍能读到自己的知识。
    assert!(
        !wa.tool("WikiRead", &serde_json::json!({"id": secret.meta.id}), "s")
            .await
            .unwrap()
            .failed
    );
}

#[tokio::test]
async fn retrieval_cache_does_not_leak_across_projects_or_versions() {
    let env = Env::new();
    let a = env.repo("a", Some("https://h/x/a"));
    let b = env.repo("b", Some("https://h/x/b"));
    let wa = env.service(&a);
    let (e, _) = wa
        .propose(
            &propose(
                "Retry policy",
                "Clients must retry idempotent requests with backoff.",
                "project",
            ),
            "s",
        )
        .await
        .unwrap();
    approve_all(&wa, std::slice::from_ref(&e.meta.id)).await;
    let prompt = "implement retry with backoff for requests";
    assert_eq!(
        wa.retrieve("s", prompt, &HashMap::new())
            .await
            .unwrap()
            .status,
        "injected"
    );
    // 同一个 home、同样的 prompt：另一个项目不能命中前一个项目的缓存结果。
    let wb = env.service(&b);
    assert_ne!(
        wb.retrieve("s", prompt, &HashMap::new())
            .await
            .unwrap()
            .status,
        "injected"
    );
    // 弃用后版本变化，缓存失效，不再注入。
    wa.review(
        "deprecate",
        std::slice::from_ref(&e.meta.id),
        Some("obsolete"),
        None,
        "test",
    )
    .await
    .unwrap();
    assert_ne!(
        wa.retrieve("s", prompt, &HashMap::new())
            .await
            .unwrap()
            .status,
        "injected"
    );
}

#[tokio::test]
async fn identity_policies() {
    let env = Env::new();
    // 复制/移动的同一仓库（同 remote）→ 同一项目；fork（不同 remote）→ 不同项目。
    let one = resolve_project(&env.repo("one", Some("git@github.com:Org/Repo.git")), None).await;
    let copy = resolve_project(
        &env.repo("renamed-copy", Some("https://github.com/Org/Repo")),
        None,
    )
    .await;
    let fork = resolve_project(
        &env.repo("fork", Some("git@github.com:someone/Repo.git")),
        None,
    )
    .await;
    assert_eq!(one.kind, IdentityKind::GitRemote);
    assert_eq!(one.id, copy.id);
    assert_ne!(one.id, fork.id);
    // 无 remote 的 Git：按主仓库目录；无 Git：按路径；带 workspace identity 的无 Git 目录按 identity。
    let local = resolve_project(&env.repo("local", None), None).await;
    assert_eq!(local.kind, IdentityKind::GitLocal);
    let plain = resolve_project(&env.plain("plain"), None).await;
    assert_eq!(plain.kind, IdentityKind::Path);
    let remote = resolve_project(&env.plain("plain"), Some("remote:ssh:host:22:me:/srv/app")).await;
    assert_eq!(remote.kind, IdentityKind::WorkspaceIdentity);
    assert_ne!(
        remote.id, plain.id,
        "remote identity never merges with a local path project"
    );
    // 子目录属于所在仓库；嵌套仓库最近者胜。
    let main = env.repo("mono", Some("https://h/org/mono"));
    std::fs::create_dir_all(main.join("pkg/src")).unwrap();
    assert_eq!(
        resolve_project(&main.join("pkg/src"), None).await.id,
        resolve_project(&main, None).await.id
    );
    let nested = main.join("vendor/lib");
    std::fs::create_dir_all(nested.join(".git")).unwrap();
    std::fs::write(
        nested.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://h/org/lib\n",
    )
    .unwrap();
    assert_ne!(
        resolve_project(&nested, None).await.id,
        resolve_project(&main, None).await.id
    );
    // 多工作树：gitfile → worktree gitdir → commondir 指回主仓库。
    let worktree = env.plain("wt");
    let wt_git = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&wt_git).unwrap();
    std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", wt_git.display()),
    )
    .unwrap();
    assert_eq!(
        resolve_project(&worktree, None).await.id,
        resolve_project(&main, None).await.id
    );
    // 符号链接指向同一目录 → 同一项目。
    #[cfg(unix)]
    {
        let link = env.work.path().join("link-to-plain");
        std::os::unix::fs::symlink(env.plain("plain"), &link).unwrap();
        assert_eq!(resolve_project(&link, None).await.id, plain.id);
    }
}

#[tokio::test]
async fn switching_workspaces_rebinds_scope() {
    let env = Env::new();
    let a = env.repo("a", Some("https://h/o/a"));
    let b = env.repo("b", Some("https://h/o/b"));
    let wa = env.service(&a);
    let (e, _) = wa
        .propose(
            &propose(
                "A only rule",
                "Module alpha handlers must validate tenant ids.",
                "project",
            ),
            "s",
        )
        .await
        .unwrap();
    approve_all(&wa, std::slice::from_ref(&e.meta.id)).await;
    // 每个 workspace 一个服务实例（runtime 按 workspace 构建），切换后作用域随之切换。
    let status_b = env.service(&b).status().await.unwrap();
    let project_b = status_b["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["scope"] == "project")
        .unwrap()
        .clone();
    assert_eq!(project_b["entries"], 0);
    assert_ne!(
        status_b["project"]["id"],
        wa.status().await.unwrap()["project"]["id"]
    );
    assert_eq!(
        wa.store_for(&wa.settings().await, Scope::Project)
            .await
            .project_id
            .as_deref(),
        Some(wa.project().await.id.as_str())
    );
}
