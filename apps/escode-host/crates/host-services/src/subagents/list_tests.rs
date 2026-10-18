//! `subagents.list` 的发现语义（`rust-u2-subagents.md` §6.5）：真临时 home。
use super::{ListParams, list};
use crate::subagents::SubagentsService;
use crate::subagents::{state, storage};
use serde_json::Value;

fn temp_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    (dir, home)
}

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn agent_markdown(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n正文")
}

async fn run(home: &std::path::Path, workspace: &str, mode: Option<&str>) -> Value {
    list(
        home,
        ListParams {
            workspace_path: workspace,
            mode,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap()
}

fn names(agents: &Value) -> Vec<String> {
    let mut names: Vec<String> = agents
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn built_ins_are_always_present_and_carry_their_overrides() {
    let (_dir, home) = temp_home();
    let answer = run(&home, "/w", None).await;
    let agents = answer["agents"].as_array().unwrap();
    assert_eq!(names(&answer["agents"]), vec!["Explore", "general-purpose"]);
    let general = agents
        .iter()
        .find(|agent| agent["name"].as_str() == Some("general-purpose"))
        .unwrap();
    assert_eq!(
        general["id"],
        Value::from("built-in:built-in:general-purpose")
    );
    assert_eq!(general["color"], Value::from("blue"));
    assert_eq!(general["readOnly"], Value::Bool(true));
    assert_eq!(general["tools"], serde_json::json!(["*"]));
    assert!(general["modelSelection"].is_null(), "无覆盖时为 null");

    // 写入内置覆盖后回填到 modelSelection 与 modelSelectionOverride。
    let service = SubagentsService::new(home.clone());
    service
        .set_built_in_model_override(
            "general-purpose",
            Some(&serde_json::json!({"providerId": "p", "modelId": "m"})),
        )
        .await
        .unwrap();
    let answer = run(&home, "/w", None).await;
    let general = answer["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"].as_str() == Some("general-purpose"))
        .unwrap();
    assert_eq!(general["modelSelection"]["modelId"], Value::from("m"));
    assert_eq!(
        general["modelSelectionOverride"]["modelId"],
        Value::from("m")
    );
}

/// 自定义 profile **只扫第一层**；子目录里的 md 不参与，`.markdown` 也算。
#[tokio::test]
async fn user_and_workspace_agents_are_discovered_at_the_first_level_only() {
    let (_dir, home) = temp_home();
    let workspace = home.join("workspace");
    let user_root = storage::user_agents_root(&home).await;
    write(
        &user_root.join("alpha.md"),
        &agent_markdown("alpha", "user alpha"),
    );
    write(
        &user_root.join("beta.markdown"),
        &agent_markdown("beta", "user beta"),
    );
    write(
        &user_root.join("nested").join("gamma.md"),
        &agent_markdown("gamma", "不该被发现"),
    );
    // 缺 description 的文件报诊断、不进结果。
    write(&user_root.join("broken.md"), "---\nname: broken\n---\nbody");
    write(
        &workspace
            .join(".escode")
            .join("agents")
            .join("project-one.md"),
        &agent_markdown("project-one", "项目级"),
    );

    let answer = run(&home, &workspace.to_string_lossy(), None).await;
    // `userAgents` 按 `source === "user"` 过滤，因此**也包含** workspace 级（TS 同）。
    assert_eq!(
        names(&answer["userAgents"]),
        vec!["alpha", "beta", "project-one"]
    );
    assert!(
        !names(&answer["userAgents"]).contains(&"gamma".to_owned()),
        "子目录里的 md 不该被发现"
    );
    let diagnostics = answer["diagnostics"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"].as_str()
                == Some("agent_missing_required_frontmatter")),
        "{diagnostics:?}"
    );
    let project = answer["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"].as_str() == Some("project-one"))
        .unwrap();
    assert_eq!(project["scope"], Value::from("workspace"));
    assert_eq!(
        project["projectPath"],
        Value::from(workspace.to_string_lossy().to_string())
    );

    // `settingsUserOnly` 不含工作区 agent。
    let answer = run(
        &home,
        &workspace.to_string_lossy(),
        Some("settingsUserOnly"),
    )
    .await;
    assert!(!names(&answer["agents"]).contains(&"project-one".to_owned()));
}

/// 同名时后者覆盖前者（workspace 排在 user 之后），且保持单条。
#[tokio::test]
async fn runtime_precedence_keeps_one_entry_per_name() {
    let (_dir, home) = temp_home();
    let workspace = home.join("workspace");
    write(
        &storage::user_agents_root(&home).await.join("dup.md"),
        &agent_markdown("dup", "user 版本"),
    );
    write(
        &workspace.join(".escode").join("agents").join("dup.md"),
        &agent_markdown("dup", "project 版本"),
    );
    let answer = run(&home, &workspace.to_string_lossy(), None).await;
    let matching: Vec<&Value> = answer["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|agent| agent["name"].as_str() == Some("dup"))
        .collect();
    assert_eq!(matching.len(), 1, "同名只保留一条");
    assert_eq!(matching[0]["description"], Value::from("project 版本"));
}

/// 禁用集只影响 user scope；内置与插件恒为启用。
#[tokio::test]
async fn disabled_state_only_applies_to_user_scope() {
    let (_dir, home) = temp_home();
    let user_root = storage::user_agents_root(&home).await;
    write(&user_root.join("off.md"), &agent_markdown("off", "d"));
    write(&user_root.join("on.md"), &agent_markdown("on", "d"));
    let service = SubagentsService::new(home.clone());
    service.set_enabled("user:user:off", false).await.unwrap();
    // 把**内置** id 也塞进禁用集：scope 判断必须让它保持启用（只有 user scope 受影响）。
    service
        .set_enabled("built-in:built-in:general-purpose", false)
        .await
        .unwrap();
    let answer = run(&home, "/w", None).await;
    let find = |name: &str| {
        answer["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["name"].as_str() == Some(name))
            .cloned()
            .unwrap()
    };
    assert_eq!(find("off")["enabled"], Value::Bool(false));
    assert_eq!(find("on")["enabled"], Value::Bool(true));
    assert_eq!(
        find("general-purpose")["enabled"],
        Value::Bool(true),
        "内置不受禁用集影响"
    );
    // 状态文件确实落盘（证明走的是同一份 agents-state.json）。
    let state = state::read_state(&storage::state_file(&home).await)
        .await
        .unwrap();
    assert_eq!(
        state["disabledAgentIds"],
        serde_json::json!(["built-in:built-in:general-purpose", "user:user:off"])
    );
}

/// 插件 agent：清单 `agents` 声明目录、递归发现、带插件名前缀与 `plugin:` id；
/// 裸名唯一时额外产出一条别名。
#[tokio::test]
async fn plugin_agents_are_discovered_with_prefixed_names_and_bare_aliases() {
    let (_dir, home) = temp_home();
    let plugin = home.join("plugin-root");
    write(
        &plugin.join(".escode-plugin").join("plugin.json"),
        r#"{"name":"demo","agents":["defs"]}"#,
    );
    write(
        &plugin.join("defs").join("review-deep.md"),
        &agent_markdown("reviewer", "插件 reviewer"),
    );
    let installed = serde_json::json!({
        "plugins": [{
            "id": "demo@inline",
            "installPath": plugin.to_string_lossy(),
            "name": "demo"
        }]
    });
    write(
        &home
            .join(".escode")
            .join("cli")
            .join("plugins")
            .join("installed_plugins.json"),
        &installed.to_string(),
    );
    write(
        &home.join(".escode").join("cli").join("config.json"),
        r#"{"plugins":{"enabledPlugins":{"demo@inline":true}}}"#,
    );

    let answer = run(&home, "/w", None).await;
    let profiles = answer["pluginAgents"].as_array().unwrap();
    assert_eq!(profiles.len(), 1, "UI 投影只有带前缀的一条: {profiles:?}");
    let agent = &profiles[0];
    assert_eq!(agent["name"], Value::from("demo:reviewer"));
    assert_eq!(agent["id"], Value::from("plugin:demo@inline:reviewer"));
    assert_eq!(agent["source"], Value::from("plugin"));
    assert_eq!(agent["pluginName"], Value::from("demo"));
    assert_eq!(agent["readOnly"], Value::Bool(true));
    // 运行时集里多一条裸名别名。
    let runtime = answer["agents"].as_array().unwrap();
    assert!(
        runtime
            .iter()
            .any(|candidate| candidate["name"].as_str() == Some("reviewer")),
        "裸名唯一时应产出别名: {:?}",
        names(&answer["agents"])
    );

    // 插件被关掉：不再贡献。
    write(
        &home.join(".escode").join("cli").join("config.json"),
        r#"{"plugins":{"enabledPlugins":{"demo@inline":false}}}"#,
    );
    let answer = run(&home, "/w", None).await;
    assert!(answer["pluginAgents"].as_array().unwrap().is_empty());
}

/// 裸名与内置名冲突时不产别名，并报 `agent_ambiguous_name`。
#[tokio::test]
async fn plugin_bare_name_conflicting_with_builtin_reports_a_diagnostic() {
    let (_dir, home) = temp_home();
    let plugin = home.join("plugin-root");
    write(
        &plugin.join(".escode-plugin").join("plugin.json"),
        r#"{"name":"demo","commands":[]}"#,
    );
    write(
        &plugin.join("agents").join("agent.md"),
        &agent_markdown("Explore", "插件版 Explore"),
    );
    write(
        &home
            .join(".escode")
            .join("cli")
            .join("plugins")
            .join("installed_plugins.json"),
        &serde_json::json!({"plugins": [{
            "id": "demo@inline",
            "installPath": plugin.to_string_lossy(),
            "name": "demo"
        }]})
        .to_string(),
    );
    write(
        &home.join(".escode").join("cli").join("config.json"),
        r#"{"plugins":{"enabledPlugins":{"demo@inline":true}}}"#,
    );
    let answer = run(&home, "/w", None).await;
    assert!(
        answer["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| {
                diagnostic["code"].as_str() == Some("agent_ambiguous_name")
                    && diagnostic["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("conflicts with an existing profile")
            }),
        "{:?}",
        answer["diagnostics"]
    );
    // 带前缀的 profile 仍在，但没有裸名别名。
    assert_eq!(
        answer["pluginAgents"][0]["name"],
        Value::from("demo:Explore")
    );
    assert!(
        !answer["agents"].as_array().unwrap().iter().any(|agent| {
            agent["name"].as_str() == Some("Explore") && agent["source"].as_str() == Some("plugin")
        }),
        "与内置名冲突时不产别名: {:?}",
        names(&answer["agents"])
    );
}

#[tokio::test]
async fn capability_reports_user_scope_available() {
    let (_dir, home) = temp_home();
    let answer = run(&home, "/w", None).await;
    assert_eq!(
        answer["capability"]["userScopeAvailable"],
        Value::Bool(true)
    );
}
