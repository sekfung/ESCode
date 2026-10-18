//! 路径解析（`rust-u2-subagents.md` §1，TS `subagentStorage.ts`）。
use super::*;

#[tokio::test]
async fn storage_root_defaults_to_dot_escode_under_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    assert_eq!(storage_root(home).await, home.join(".escode"));
    assert_eq!(
        user_agents_root(home).await,
        home.join(".escode").join("agents")
    );
    assert_eq!(
        state_file(home).await,
        home.join(".escode").join("v2").join("agents-state.json")
    );
    assert_eq!(
        workspace_agents_root("/work"),
        Path::new("/work/.escode/agents")
    );
}

/// `config.storage.dir` 覆盖根目录：支持 `~/` 展开与绝对路径，空/空白回退缺省。
#[tokio::test]
async fn storage_dir_from_cli_config_overrides_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let config_path = home.join(".escode").join("cli").join("config.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let write_config = |dir_value: &str| {
        std::fs::write(
            &config_path,
            serde_json::json!({"storage": {"dir": dir_value}}).to_string(),
        )
        .unwrap();
    };

    write_config("~/custom-root");
    assert_eq!(storage_root(home).await, home.join("custom-root"));
    assert_eq!(
        user_agents_root(home).await,
        home.join("custom-root").join("agents")
    );

    write_config("/tmp/absolute-root");
    // 修复：有根无盘符的路径按 Node path.resolve 补当前盘符（Windows 上为 `D:\tmp\...`）；
    // std::path::absolute 同一规则，Unix 上原样。
    assert_eq!(
        storage_root(home).await,
        std::path::absolute("/tmp/absolute-root").unwrap()
    );

    // 空白与坏 JSON 都回退缺省。
    write_config("   ");
    assert_eq!(storage_root(home).await, home.join(".escode"));
    std::fs::write(&config_path, "{oops").unwrap();
    assert_eq!(storage_root(home).await, home.join(".escode"));
}
