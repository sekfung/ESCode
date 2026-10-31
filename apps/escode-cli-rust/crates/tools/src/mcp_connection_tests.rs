use super::ResultBudget;
use serde_json::json;

fn budget(name: &str, raw: serde_json::Value, authority: Option<&str>) -> ResultBudget {
    ResultBudget::resolve(name, &raw, authority)
}

#[tokio::test]
async fn normal_budget_keeps_exact_limit() {
    let content = "a".repeat(50_000);
    assert!(
        budget("local.big", json!({}), None)
            .apply(&content, None)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn normal_budget_truncates_ascii_with_node_suffix() {
    let content = "x".repeat(60_000);
    let truncated = budget("local.big", json!({}), None)
        .apply(&content, None)
        .await
        .expect("over budget");
    assert_eq!(truncated.len(), 50_000);
    assert!(truncated.ends_with(
            "\n\n[Tool output truncated by resultBudget: originalBytes=60000, maxModelBytes=50000, strategy=truncate]"
        ));
}

#[tokio::test]
async fn normal_budget_truncates_unicode_on_char_boundary() {
    let content = "界".repeat(20_000);
    let truncated = budget("local.big", json!({}), None)
        .apply(&content, None)
        .await
        .expect("over budget");
    let at = truncated
        .find("\n\n[Tool output truncated")
        .expect("suffix");
    assert!(truncated.len() <= 50_000);
    assert!(truncated[..at].chars().all(|c| c == '界'));
}

#[tokio::test]
async fn cua_budget_keeps_legacy_budget_without_authority() {
    let content = "x".repeat(70_000);
    let truncated = budget("escode-cua", json!({"type": "stdio", "command": "x"}), None)
        .apply(&content, None)
        .await
        .expect("over budget");
    assert!(truncated.ends_with("\n[MCP result truncated]"));
    assert_eq!(
        truncated.trim_end_matches("\n[MCP result truncated]").len(),
        64 * 1024
    );
}

#[tokio::test]
async fn official_cua_budget_uses_256_kib_only_with_matching_authority() {
    let raw = json!({"type": "stdio", "command": "x", "env": {
        "ESCODE_PLUGIN_ID": " computer-use@zcode-plugins-official ",
        "ESCODE_CUA_PLUGIN_AUTHORITY": " authority ",
    }});
    let content = "x".repeat(300_000);
    let truncated = budget(
        "plugin:escode-cua:computer-use",
        raw.clone(),
        Some("authority"),
    )
    .apply(&content, None)
    .await
    .expect("over budget");
    assert_eq!(truncated.len(), 256 * 1024);
    assert!(truncated.ends_with(
            "\n\n[Tool output truncated by resultBudget: originalBytes=300000, maxModelBytes=262144, strategy=truncate]"
        ));
    // 同一配置在 authority 缺失或错误时不能放宽。
    for authority in [None, Some("other")] {
        let truncated = budget("plugin:escode-cua:computer-use", raw.clone(), authority)
            .apply(&content, None)
            .await
            .expect("over budget");
        assert!(truncated.ends_with("\n[MCP result truncated]"));
        assert_eq!(
            truncated.trim_end_matches("\n[MCP result truncated]").len(),
            64 * 1024
        );
    }
}

#[tokio::test]
async fn node_repl_keeps_artifact_budget_even_with_cua_authority_env() {
    let raw = json!({"type": "stdio", "command": "x", "env": {
        "ESCODE_PLUGIN_ID": "computer-use@zcode-plugins-official",
        "ESCODE_CUA_PLUGIN_AUTHORITY": "authority",
    }});
    let content = "x".repeat(70_000);
    let truncated = budget("node_repl", raw, Some("authority"))
        .apply(&content, None)
        .await
        .expect("over budget");
    assert!(truncated.ends_with(
            "\n\n[Tool output truncated by resultBudget: originalBytes=70000, maxModelBytes=65536, strategy=artifact]"
        ));
}

#[tokio::test]
async fn node_repl_without_artifact_target_falls_back_to_tail_truncation() {
    let content = "x".repeat(70_000);
    let truncated = budget("node_repl", json!({"type": "stdio", "command": "x"}), None)
        .apply(&content, None)
        .await
        .expect("over budget");
    assert!(truncated.starts_with('x'));
    assert!(truncated.ends_with(
            "\n\n[Tool output truncated by resultBudget: originalBytes=70000, maxModelBytes=65536, strategy=artifact]"
        ));
    assert_eq!(truncated.len(), 64 * 1024);
}

#[tokio::test]
async fn node_repl_writes_an_artifact_and_returns_the_persisted_envelope() {
    let root = std::env::temp_dir().join(format!("escode-mcp-artifact-{}", escode_cli_host::id()));
    let target = super::ImageArtifacts {
        root: &root,
        session: "sess_test",
        call_id: "call-1",
    };
    let content = format!("{}{}", "a".repeat(66_000), "TAIL");
    let envelope = budget("node_repl", json!({"type": "stdio", "command": "x"}), None)
        .apply(&content, Some(target))
        .await
        .expect("over budget");
    assert!(
        envelope
            .starts_with("<persisted-output>\nOutput too large (66 KB). Full output saved to: ")
    );
    assert!(
        envelope.ends_with("\n...\n</persisted-output>"),
        "{envelope}"
    );
    let path = envelope
        .lines()
        .nth(1)
        .and_then(|line| line.split("Full output saved to: ").nth(1))
        .expect("artifact path");
    assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    assert!(path.ends_with(".json"));
    let _ = std::fs::remove_dir_all(&root);
}
