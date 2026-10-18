//! 写三法（`rust-u2-subagents.md` §7）：真临时 home。
use super::*;
use serde_json::json;

fn service() -> (tempfile::TempDir, SubagentsService) {
    let dir = tempfile::tempdir().unwrap();
    let service = SubagentsService::new(dir.path().to_path_buf());
    (dir, service)
}

fn create_params(name: &str, prompt: &str) -> Value {
    json!({"config": {
        "name": name,
        "description": "演示 agent",
        "systemPrompt": prompt,
        "tools": ["Read"]
    }})
}

#[tokio::test]
async fn create_writes_a_parseable_file_and_returns_the_summary() {
    let (dir, service) = service();
    let answer = service
        .create_agent(&create_params("reviewer", "你是审查员。"))
        .await
        .unwrap();
    let agent = &answer["agent"];
    let path = dir
        .path()
        .join(".escode")
        .join("agents")
        .join("reviewer.md");
    assert_eq!(agent["path"], json!(path.to_string_lossy()));
    assert_eq!(agent["id"], json!("user:user:reviewer"));
    assert_eq!(agent["scope"], json!("user"));
    assert_eq!(agent["source"], json!("user"));
    assert_eq!(agent["readOnly"], json!(false));
    assert_eq!(agent["systemPrompt"], json!("你是审查员。"));
    assert_eq!(agent["tools"], json!(["Read"]), "Settings 侧不剥参数");
    // 落盘内容可被 list 重新发现。
    let markdown = std::fs::read_to_string(&path).unwrap();
    assert!(
        markdown.starts_with("---\nname: \"reviewer\"\n"),
        "{markdown}"
    );
    let listed = super::super::list::list(
        dir.path(),
        super::super::list::ListParams {
            workspace_path: "/w",
            mode: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    assert!(
        listed["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|candidate| candidate["name"] == json!("reviewer")),
        "新建的 agent 必须立刻出现在 list 里"
    );
}

#[tokio::test]
async fn create_rejects_duplicates_reserved_names_and_bad_configs() {
    let (_dir, service) = service();
    service
        .create_agent(&create_params("reviewer", "p"))
        .await
        .unwrap();
    let error = service
        .create_agent(&create_params("reviewer", "p"))
        .await
        .unwrap_err();
    assert_eq!(error, "Agent file \"reviewer.md\" already exists");

    for (label, params, expected) in [
        (
            "内置名",
            json!({"config": {"name": "Explore", "description": "d", "systemPrompt": "p"}}),
            "Agent name \"Explore\" is reserved by a built-in agent",
        ),
        (
            "名字太短",
            json!({"config": {"name": "ab", "description": "d", "systemPrompt": "p"}}),
            "Name must be between 3 and 50 characters",
        ),
        (
            "非法字符",
            json!({"config": {"name": "a b", "description": "d", "systemPrompt": "p"}}),
            "Name can only contain letters, numbers, and hyphens",
        ),
        (
            "缺描述",
            json!({"config": {"name": "abc", "description": "  ", "systemPrompt": "p"}}),
            "Description is required",
        ),
        (
            "缺正文",
            json!({"config": {"name": "abc", "description": "d", "systemPrompt": " "}}),
            "System prompt is required",
        ),
        (
            "工作区缺路径",
            json!({"scope": "workspace", "config": {"name": "abc", "description": "d", "systemPrompt": "p"}}),
            "Workspace path is required for workspace subagents",
        ),
    ] {
        let error = service.create_agent(&params).await.unwrap_err();
        assert_eq!(error, expected, "{label}");
    }
}

#[tokio::test]
async fn workspace_scope_writes_under_the_workspace_and_carries_project_path() {
    let (dir, service) = service();
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let answer = service
        .create_agent(&json!({
            "scope": "workspace",
            "workspacePath": workspace.to_string_lossy(),
            "config": {"name": "local-one", "description": "d", "systemPrompt": "p"}
        }))
        .await
        .unwrap();
    let path = workspace
        .join(".escode")
        .join("agents")
        .join("local-one.md");
    assert!(path.exists());
    assert_eq!(answer["agent"]["scope"], json!("workspace"));
    assert_eq!(
        answer["agent"]["projectPath"],
        json!(workspace.to_string_lossy())
    );
    assert_eq!(answer["agent"]["id"], json!("user:workspace:local-one"));
}

/// 改名必须把禁用态迁到新 id，否则刚禁用的 agent 会重新启用。
#[tokio::test]
async fn rename_migrates_the_disabled_state() {
    let (dir, service) = service();
    let created = service
        .create_agent(&create_params("old-name", "p"))
        .await
        .unwrap();
    let old_id = created["agent"]["id"].as_str().unwrap().to_owned();
    service.set_enabled(&old_id, false).await.unwrap();

    let answer = service
        .update_agent(&json!({
            "agentId": old_id,
            "oldFilePath": created["agent"]["path"],
            "config": {"name": "new-name", "description": "d", "systemPrompt": "p"}
        }))
        .await
        .unwrap();
    let new_id = answer["agent"]["id"].as_str().unwrap().to_owned();
    assert_eq!(new_id, "user:user:new-name");
    assert!(
        !dir.path()
            .join(".escode")
            .join("agents")
            .join("old-name.md")
            .exists(),
        "旧文件应删除"
    );
    assert!(
        dir.path()
            .join(".escode")
            .join("agents")
            .join("new-name.md")
            .exists()
    );
    let state =
        super::super::state::read_state(&super::super::storage::state_file(dir.path()).await)
            .await
            .unwrap();
    assert_eq!(
        state["disabledAgentIds"],
        json!([new_id]),
        "禁用态跟到新 id"
    );
}

#[tokio::test]
async fn update_rejects_renaming_onto_an_existing_file() {
    let (dir, service) = service();
    service
        .create_agent(&create_params("taken", "p"))
        .await
        .unwrap();
    let source = service
        .create_agent(&create_params("source", "p"))
        .await
        .unwrap();
    let error = service
        .update_agent(&json!({
            "agentId": source["agent"]["id"],
            "oldFilePath": source["agent"]["path"],
            "config": {"name": "taken", "description": "d", "systemPrompt": "p"}
        }))
        .await
        .unwrap_err();
    assert_eq!(error, "Agent file \"taken.md\" already exists");
    // 撞名时不能动旧文件。
    assert!(
        dir.path()
            .join(".escode")
            .join("agents")
            .join("source.md")
            .exists()
    );
}

#[tokio::test]
async fn delete_removes_the_file_and_the_disabled_entry() {
    let (dir, service) = service();
    let created = service
        .create_agent(&create_params("gone", "p"))
        .await
        .unwrap();
    let id = created["agent"]["id"].as_str().unwrap().to_owned();
    let path = created["agent"]["path"].as_str().unwrap().to_owned();
    service.set_enabled(&id, false).await.unwrap();
    service
        .delete_agent(&json!({"filePath": path, "agentId": id}))
        .await
        .unwrap();
    assert!(!std::path::Path::new(&path).exists());
    let state =
        super::super::state::read_state(&super::super::storage::state_file(dir.path()).await)
            .await
            .unwrap();
    assert_eq!(state["disabledAgentIds"], json!([]), "删了就不该留悬空键");
    // 再删一次：文件不存在当作成功。
    service
        .delete_agent(&json!({"filePath": path, "agentId": id}))
        .await
        .unwrap();
}
