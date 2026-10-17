//! `commands.list` 的发现语义（`rust-u2-commands.md` §1/§6）：真临时 home。
use super::*;

fn temp_home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    (dir, home)
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

async fn command_names(home: &Path, workspace: Option<&str>) -> Vec<String> {
    let answer = list(
        home,
        ListParams {
            agent_source: None,
            workspace_path: workspace,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    let mut names: Vec<String> = answer["userCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn user_commands_are_discovered_recursively_with_hidden_entries_skipped() {
    let (_dir, home) = temp_home();
    let root = home.join(".escode/commands");
    write(&root.join("plain.md"), "正文");
    write(
        &root.join("group/nested.md"),
        "---\ndescription: 说明\n---\n正文",
    );
    write(&root.join(".hidden.md"), "不该出现");
    write(&root.join("group/.hidden.md"), "也不该出现");

    assert_eq!(
        command_names(&home, None).await,
        vec!["/group/nested", "/plain"]
    );

    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    let plain = answer["userCommands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == serde_json::json!("/plain"))
        .expect("plain 在结果里");
    assert_eq!(plain["source"], serde_json::json!("user"));
    assert_eq!(plain["scope"], serde_json::json!("global"));
    assert_eq!(plain["enabled"], serde_json::json!(true));
    assert_eq!(plain["agentSource"], serde_json::json!("escodeAgent"));
    assert_eq!(
        plain["id"],
        serde_json::json!("escodeAgent:escode:global:/plain")
    );
    assert_eq!(
        answer["capability"]["userScopeAvailable"],
        serde_json::json!(true)
    );
}

/// `.escode` 是强优先级来源：同 scope 下读到有效命令就不再看 `.agents`。
#[tokio::test]
async fn escode_directory_shadows_agents_and_agents_is_the_fallback() {
    let (_dir, home) = temp_home();
    write(&home.join(".escode/commands/from-escode.md"), "a");
    write(&home.join(".agents/commands/from-agents.md"), "b");
    assert_eq!(command_names(&home, None).await, vec!["/from-escode"]);

    // `.escode` 没有命令时 `.agents` 顶上。
    std::fs::remove_file(home.join(".escode/commands/from-escode.md")).unwrap();
    assert_eq!(command_names(&home, None).await, vec!["/from-agents"]);
}

/// 先 workspace 后 user，同名保留**先出现**的（workspace 胜）。
#[tokio::test]
async fn workspace_commands_precede_user_commands_in_dedup() {
    let (_dir, home) = temp_home();
    let workspace = home.join("workspace");
    write(&home.join(".escode/commands/dup.md"), "user 版本");
    write(&workspace.join(".escode/commands/dup.md"), "project 版本");
    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: Some(&workspace.to_string_lossy()),
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    let users = answer["userCommands"].as_array().unwrap();
    assert_eq!(users.len(), 1, "同名只保留一条");
    assert_eq!(users[0]["scope"], serde_json::json!("project"));
    assert_eq!(users[0]["prompt"], serde_json::json!("project 版本"));
    assert_eq!(
        users[0]["projectPath"],
        serde_json::json!(workspace.to_string_lossy())
    );
    assert_eq!(
        users[0]["id"],
        serde_json::json!("escodeAgent:escode:project:/dup")
    );
}

/// 禁用态来自 CLI config 的覆盖表。
#[tokio::test]
async fn disabled_commands_report_enabled_false() {
    let (_dir, home) = temp_home();
    let file = home.join(".escode/commands/off.md");
    write(&file, "x");
    let config = serde_json::json!({
        "command": {file.to_string_lossy().to_string(): {"enable": false}}
    });
    std::fs::create_dir_all(home.join(".escode/cli")).unwrap();
    std::fs::write(
        home.join(".escode/cli/config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    assert_eq!(
        answer["userCommands"][0]["enabled"],
        serde_json::json!(false)
    );
}

/// 插件命令：清单 `commands` 指向的子目录、`plugin:` id、按 name 排序、设置里的启停生效。
#[tokio::test]
async fn plugin_commands_come_from_configured_roots_and_respect_enablement() {
    let (_dir, home) = temp_home();
    let plugin = home.join("my-plugin");
    write(
        &plugin.join(".escode-plugin/plugin.json"),
        r#"{"name":"demo","commands":["cmds"]}"#,
    );
    write(&plugin.join("cmds/beta.md"), "beta");
    write(&plugin.join("cmds/alpha.md"), "alpha");
    // 不在 commands 列表里的目录不参与。
    write(&plugin.join("other/gamma.md"), "gamma");

    let config = serde_json::json!({"plugins": {"dirs": [plugin.to_string_lossy()]}});
    std::fs::create_dir_all(home.join(".escode/cli")).unwrap();
    std::fs::write(
        home.join(".escode/cli/config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();

    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    let plugins = answer["pluginCommands"].as_array().unwrap();
    let names: Vec<&str> = plugins
        .iter()
        .map(|command| command["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["/alpha", "/beta"],
        "按 name 排序、只收清单指向的目录"
    );
    let alpha = &plugins[0];
    assert_eq!(alpha["source"], serde_json::json!("plugin"));
    assert_eq!(alpha["scope"], serde_json::json!("global"));
    assert_eq!(alpha["pluginName"], serde_json::json!("demo"));
    assert_eq!(alpha["pluginMarketplace"], serde_json::json!("inline"));
    assert_eq!(alpha["pluginEnabled"], serde_json::json!(true));
    assert_eq!(alpha["enabled"], serde_json::json!(true));
    assert!(
        alpha["id"]
            .as_str()
            .unwrap()
            .starts_with("plugin:inline:demo:/alpha:"),
        "{alpha}"
    );
    // `commands` 汇总 = 用户 + 插件。
    assert_eq!(answer["commands"].as_array().unwrap().len(), plugins.len());

    // 插件被关掉：不再贡献命令。
    let config = serde_json::json!({
        "plugins": {"dirs": [plugin.to_string_lossy()], "enabledPlugins": {"demo@inline": false}}
    });
    std::fs::write(
        home.join(".escode/cli/config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    assert!(answer["pluginCommands"].as_array().unwrap().is_empty());

    // 全局关闭插件扫描。
    let config =
        serde_json::json!({"plugins": {"dirs": [plugin.to_string_lossy()], "enabled": false}});
    std::fs::write(
        home.join(".escode/cli/config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    assert!(answer["pluginCommands"].as_array().unwrap().is_empty());
}

/// 官方缓存里的内置插件被「卸载」后只写在 `suppressedBuiltins`：desktop 直接扫 cache 必须同样跳过。
#[tokio::test]
async fn suppressed_builtins_are_skipped_even_when_the_cache_still_has_them() {
    let (_dir, home) = temp_home();
    let version_root =
        home.join(".escode/cli/plugins/cache/zcode-plugins-official/visualize/1.0.0");
    write(
        &version_root.join(".escode-plugin/plugin.json"),
        r#"{"name":"visualize","commands":["commands"]}"#,
    );
    write(&version_root.join("commands/graph.md"), "graph");
    let config = serde_json::json!({
        "plugins": {
            "enabledPlugins": {"visualize@zcode-plugins-official": true},
            "suppressedBuiltins": ["visualize@zcode-plugins-official"]
        }
    });
    std::fs::create_dir_all(home.join(".escode/cli")).unwrap();
    std::fs::write(
        home.join(".escode/cli/config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    assert!(
        answer["pluginCommands"].as_array().unwrap().is_empty(),
        "被卸载的内置插件不能从 cache 贡献命令"
    );

    // 去掉 suppressed 后同一份 cache 就能贡献（证明上一条不是别的原因导致的空）。
    let config = serde_json::json!({
        "plugins": {"enabledPlugins": {"visualize@zcode-plugins-official": true}}
    });
    std::fs::write(
        home.join(".escode/cli/config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let answer = list(
        &home,
        ListParams {
            agent_source: None,
            workspace_path: None,
        },
        &|a, b| a.cmp(b),
    )
    .await
    .unwrap();
    assert_eq!(answer["pluginCommands"].as_array().unwrap().len(), 1);
    assert_eq!(
        answer["pluginCommands"][0]["pluginMarketplace"],
        serde_json::json!("zcode-plugins-official")
    );
}
