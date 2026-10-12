//! 存储布局（spec §3）：Global Wiki 在 `<storage>/wiki`；Project Wiki 在项目工作区 `.escode/wiki/`，
//! 只把知识本体入库，只读路径不在仓库里生成文件，固定 Project ID 让其他机器的克隆仍可用。
mod common;
use common::*;
use escode_cli_domain::wiki::entry::Scope;
use escode_cli_domain::wiki::tools::ProposeInput;
use std::collections::HashMap;
use std::path::Path;

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

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[tokio::test]
async fn global_root_is_storage_wiki_without_cli_segment() {
    let env = Env::new();
    let repo = env.repo("app", Some("https://h/o/layout1"));
    let settings = env.service(&repo).settings().await;
    let expected = match std::env::var("ESCODE_STORAGE_DIR") {
        Ok(dir) if !dir.is_empty() => Path::new(&dir).join("wiki"),
        _ => env.home.path().join(".escode/wiki"),
    };
    assert_eq!(settings.root, expected);

    // 用户层 storage.dir 同样直接拼 /wiki（ESCODE_STORAGE_DIR 未设置时生效）。
    if std::env::var("ESCODE_STORAGE_DIR").is_err() {
        let storage = env.work.path().join("data");
        env.user_config(serde_json::json!({"storage": {"dir": storage}}));
        assert_eq!(
            env.service(&repo).settings().await.root,
            storage.join("wiki")
        );
    }

    // Global 存储即根目录本身：条目直接在 <root>/entries 下。
    let wiki = env.service(&repo);
    let (entry, _) = wiki
        .propose(
            &propose("Logging", "Services must log with trace ids.", "global"),
            "s",
        )
        .await
        .unwrap();
    let root = wiki.settings().await.root;
    assert!(
        root.join("entries")
            .join(format!("{}.md", entry.meta.id))
            .exists()
    );
    assert!(!root.join("global").exists() && !root.join("projects").exists());
}

#[tokio::test]
async fn project_wiki_lives_in_the_workspace_and_only_knowledge_is_versioned() {
    let env = Env::new();
    let repo = env.repo("app", Some("https://h/o/layout2"));
    let nested = repo.join("src/module");
    std::fs::create_dir_all(&nested).unwrap();
    // 从子目录打开也落在仓库根的 .escode/wiki。
    let wiki = env.service(&nested);
    let (entry, _) = wiki
        .propose(
            &propose(
                "Error envelope",
                "APIs must return { code, message }.",
                "project",
            ),
            "s",
        )
        .await
        .unwrap();
    let dir = repo.join(".escode/wiki");
    assert!(
        dir.join("entries")
            .join(format!("{}.md", entry.meta.id))
            .exists()
    );
    assert_eq!(
        wiki.store_for(&wiki.settings().await, Scope::Project)
            .await
            .root,
        dir
    );

    let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    for local in [
        "sources/",
        "ir/",
        "index.json",
        "audit.jsonl",
        ".lock/",
        "*.tmp",
    ] {
        assert!(
            ignore.lines().any(|l| l == local),
            "{local} must be ignored"
        );
    }
    assert!(
        !ignore.lines().any(|l| l.starts_with("entries")),
        "entries are versioned"
    );
    let project: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("project.json")).unwrap()).unwrap();
    assert_eq!(project["id"], wiki.project().await.id.as_str());
    assert!(
        !project
            .to_string()
            .contains(env.work.path().to_string_lossy().as_ref()),
        "project.json must not commit machine-local paths"
    );

    // 不能把项目内的 Wiki 存储当作文档导入。
    let report = wiki
        .ingest(std::slice::from_ref(&dir), Scope::Project, false)
        .await
        .unwrap();
    assert!(report.items.iter().all(|i| i.status == "skipped"));
}

#[tokio::test]
async fn read_only_operations_never_create_files_in_the_repository() {
    let env = Env::new();
    let repo = env.repo("app", Some("https://h/o/layout3"));
    let wiki = env.service(&repo);
    wiki.status().await.unwrap();
    wiki.graph(&serde_json::json!({})).await.unwrap();
    wiki.retrieve("s", "add structured logging", &HashMap::new())
        .await
        .unwrap();
    wiki.tool("WikiSearch", &serde_json::json!({"query": "logging"}), "s")
        .await
        .unwrap();
    assert!(
        !repo.join(".escode").exists(),
        "reads must not touch the repository"
    );
}

#[tokio::test]
async fn committed_knowledge_works_in_a_clone_on_another_machine() {
    let env = Env::new();
    // 无 remote：身份键含本机路径，两个位置算出的 ID 不同，必须靠入库的 project.json 统一。
    let original = env.repo("original", None);
    write(
        &original.join("docs/rules.md"),
        "# Rules\n\n## Retries\n\nClients must retry idempotent requests at most three times.\n",
    );
    let first = env.service(&original);
    let report = first
        .ingest(&[original.join("docs")], Scope::Project, true)
        .await
        .unwrap();
    assert!(report.ok());
    let id = first.project().await.id.clone();

    // 模拟另一台机器上的克隆：只带走版本管理内的文件（文档 + entries/ + project.json + .gitignore）。
    let clone = env.repo("elsewhere/clone", None);
    write(
        &clone.join("docs/rules.md"),
        &std::fs::read_to_string(original.join("docs/rules.md")).unwrap(),
    );
    let src = original.join(".escode/wiki");
    let dst = clone.join(".escode/wiki");
    copy_dir(&src.join("entries"), &dst.join("entries"));
    for file in ["project.json", ".gitignore"] {
        std::fs::copy(src.join(file), dst.join(file)).unwrap();
    }

    let computed = escode_cli_wiki::identity::resolve_project(&clone, None).await;
    assert_ne!(
        computed.id, id,
        "without project.json the clone would get another id"
    );
    let second = env.service(&clone);
    assert_eq!(
        second.project().await.id,
        id,
        "clone reuses the committed project id"
    );
    let status = second.status().await.unwrap();
    let project_scope = status["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["scope"] == "project")
        .unwrap()
        .clone();
    assert_eq!(
        project_scope["invalidFiles"], 0,
        "committed entries stay valid"
    );
    let hit = second
        .retrieve(
            "s",
            "how many times should clients retry requests",
            &HashMap::new(),
        )
        .await
        .unwrap();
    assert!(
        !hit.injected.is_empty(),
        "approved knowledge is injected in the clone"
    );

    // 克隆里重新导入同一文档：命中已提交条目，不新建、不改写。
    let before = std::fs::read_dir(dst.join("entries")).unwrap().count();
    let report = second
        .ingest(&[clone.join("docs")], Scope::Project, false)
        .await
        .unwrap();
    assert!(report.items.iter().all(|i| i.created == 0), "{report:?}");
    assert_eq!(
        std::fs::read_dir(dst.join("entries")).unwrap().count(),
        before
    );
}

#[tokio::test]
async fn desktop_import_request_lands_in_the_chosen_scope() {
    // Desktop「导入文档」对话框发出的 v4/wiki/manage ingest：作用域决定写入哪个存储，
    // items 带 UI 解析的字段（path/status/created/updated/proposals/error）。
    let env = Env::new();
    let repo = env.repo("app", Some("https://h/o/layout5"));
    write(
        &repo.join("docs/team.md"),
        "# Team\n\n## Reviews\n\nEvery change must be reviewed by a second engineer.\n",
    );
    write(&repo.join("docs/blob.bin"), "\u{0}\u{1}binary");
    let wiki = env.service(&repo);
    let result = wiki
        .request(
            "v4/wiki/manage",
            &serde_json::json!({"action": "ingest", "paths": ["docs", "docs/blob.bin"], "scope": "global", "approveImported": false}),
        )
        .await
        .unwrap();
    let items = result["items"].as_array().unwrap();
    let md = items
        .iter()
        .find(|i| i["path"].as_str().unwrap().ends_with("team.md"))
        .unwrap();
    assert_eq!(md["status"], "imported");
    assert!(md["created"].as_u64().unwrap() >= 1);
    for key in ["updated", "proposals"] {
        assert!(md[key].is_u64(), "{key} must be present for the UI");
    }
    assert!(
        items
            .iter()
            .any(|i| i["status"] == "skipped" && i["error"].is_string()),
        "an explicitly listed unsupported file reports a reason: {items:?}"
    );
    // 写入全局，不在仓库里生成项目 Wiki；条目是候选。
    assert!(!repo.join(".escode/wiki").exists());
    let global = wiki.settings().await.root.join("entries");
    let files: Vec<_> = std::fs::read_dir(&global).unwrap().collect();
    assert!(!files.is_empty());
    let graph = wiki.graph(&serde_json::json!({})).await.unwrap();
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["type"] == "entry")
            .all(|n| n["scope"] == "global" && n["status"] == "proposal")
    );
}
