//! 状态写入面（`rust-u2-subagents.md` §3）：真临时 home。
use super::*;

fn service() -> (tempfile::TempDir, SubagentsService) {
    let dir = tempfile::tempdir().unwrap();
    let service = SubagentsService::new(dir.path().to_path_buf());
    (dir, service)
}

fn state_path(home: &std::path::Path) -> PathBuf {
    home.join(".escode").join("v2").join("agents-state.json")
}

async fn read_back(home: &std::path::Path) -> Value {
    state::read_state(&state_path(home)).await.unwrap()
}

#[tokio::test]
async fn primary_directory_is_created_and_returned_verbatim() {
    let (dir, service) = service();
    let path = service.primary_user_agents_directory().await.unwrap();
    assert_eq!(
        path,
        dir.path().join(".escode").join("agents").to_string_lossy()
    );
    assert!(std::path::Path::new(&path).is_dir());
    // 幂等。
    assert_eq!(service.primary_user_agents_directory().await.unwrap(), path);
}

/// 禁用集合去重后**升序**写回；重新启用即移除。
#[tokio::test]
async fn set_enabled_maintains_a_sorted_disabled_set() {
    let (dir, service) = service();
    // 插入序刻意「逆序」：不排序会得到 ["b","a"]，这一步就能抓到。
    service.set_enabled("b", false).await.unwrap();
    service.set_enabled("a", false).await.unwrap();
    assert_eq!(
        read_back(dir.path()).await["disabledAgentIds"],
        serde_json::json!(["a", "b"])
    );
    // 重复禁用不产生重复项。
    service.set_enabled("b", false).await.unwrap();
    assert_eq!(
        read_back(dir.path()).await["disabledAgentIds"],
        serde_json::json!(["a", "b"])
    );
    service.set_enabled("a", true).await.unwrap();
    assert_eq!(
        read_back(dir.path()).await["disabledAgentIds"],
        serde_json::json!(["b"])
    );
    // 启用不存在的 id 是幂等的。
    service.set_enabled("never", true).await.unwrap();
    assert_eq!(
        read_back(dir.path()).await["disabledAgentIds"],
        serde_json::json!(["b"])
    );
}

#[tokio::test]
async fn built_in_override_sets_and_clears_one_entry() {
    let (dir, service) = service();
    let selection = serde_json::json!({"providerId": "p", "modelId": "m"});
    service
        .set_built_in_model_override("general-purpose", Some(&selection))
        .await
        .unwrap();
    service
        .set_built_in_model_override("Explore", Some(&selection))
        .await
        .unwrap();
    let state = read_back(dir.path()).await;
    assert_eq!(
        state["builtInModelSelectionOverrides"],
        serde_json::json!({
            "general-purpose": {"providerId": "p", "modelId": "m"},
            "Explore": {"providerId": "p", "modelId": "m"}
        })
    );
    // 清除其中一个：只删该键，另一个不受影响。
    service
        .set_built_in_model_override("Explore", None)
        .await
        .unwrap();
    assert_eq!(
        read_back(dir.path()).await["builtInModelSelectionOverrides"],
        serde_json::json!({"general-purpose": {"providerId": "p", "modelId": "m"}})
    );
    // 非法选择不落盘（报错且文件不变）。
    assert!(
        service
            .set_built_in_model_override("Explore", Some(&serde_json::json!({"providerId": "p"})))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn plugin_override_requires_a_clean_plugin_id() {
    let (dir, service) = service();
    for invalid in ["demo", " plugin:x:y", "plugin:x:y "] {
        let error = service
            .set_plugin_agent_model_override(invalid, None)
            .await
            .unwrap_err();
        assert_eq!(error, "无效插件 Subagent 身份", "{invalid}");
    }
    assert!(!state_path(dir.path()).exists(), "校验失败不应创建状态文件");

    let selection = serde_json::json!({"providerId": "p", "modelId": "m", "options": {"reasoningLevel": "high"}});
    service
        .set_plugin_agent_model_override("plugin:demo:reviewer", Some(&selection))
        .await
        .unwrap();
    assert_eq!(
        read_back(dir.path()).await["pluginAgentModelSelectionOverrides"],
        serde_json::json!({
            "plugin:demo:reviewer": {
                "providerId": "p", "modelId": "m", "options": {"reasoningLevel": "high"}
            }
        })
    );
    service
        .set_plugin_agent_model_override("plugin:demo:reviewer", None)
        .await
        .unwrap();
    assert_eq!(
        read_back(dir.path()).await["pluginAgentModelSelectionOverrides"],
        serde_json::json!({})
    );
}
