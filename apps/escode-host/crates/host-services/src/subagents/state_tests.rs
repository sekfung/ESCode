//! 状态文件读写与归一（`rust-u2-subagents.md` §2）。
use super::*;

#[tokio::test]
async fn missing_or_corrupt_state_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    assert_eq!(read_state(&path).await.unwrap(), empty_state(), "文件缺失");
    std::fs::write(&path, "{oops").unwrap();
    assert_eq!(read_state(&path).await.unwrap(), empty_state(), "坏 JSON");
    std::fs::write(&path, "[]").unwrap();
    assert_eq!(read_state(&path).await.unwrap(), empty_state(), "非对象");
}

/// 落盘 = `JSON.stringify(state, null, 2)`：2 空格缩进、**无结尾换行**。
/// 键序差异（serde_json 字典序 vs TS 插入序）已登记，不影响互读。
#[tokio::test]
async fn saved_state_has_no_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2").join("agents-state.json");
    write_state(&path, &empty_state()).await.unwrap();
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(!raw.ends_with('\n'), "没有结尾换行: {raw:?}");
    assert_eq!(
        raw,
        "{\n  \"builtInModelSelectionOverrides\": {},\n  \"disabledAgentIds\": [],\n  \"pluginAgentModelSelectionOverrides\": {}\n}"
    );
}

/// 读入归一：非法条目被丢弃，未知字段与旧字段原样保留。
#[tokio::test]
async fn read_normalizes_the_known_tables_and_keeps_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "builtInModelSelectionOverrides": {
                "general-purpose": {"providerId": "p", "modelId": "m"},
                "Explore": {"providerId": "", "modelId": "m"},
                "unknown-agent": {"providerId": "p", "modelId": "m"}
            },
            "pluginAgentModelSelectionOverrides": {
                "plugin:demo:reviewer": {"providerId": "p", "modelId": "m"},
                "not-a-plugin-id": {"providerId": "p", "modelId": "m"},
                "plugin:demo:broken": {"providerId": "p"}
            },
            "disabledAgentIds": [" b ", "", 3, "a"],
            "builtInModelOverrides": {"general-purpose": {"providerId": "legacy"}},
            "futureField": {"keep": true}
        })
        .to_string(),
    )
    .unwrap();
    let state = read_state(&path).await.unwrap();
    assert_eq!(
        state["builtInModelSelectionOverrides"],
        serde_json::json!({"general-purpose": {"providerId": "p", "modelId": "m"}}),
        "只留两个合法名字且条目必须通过校验"
    );
    assert_eq!(
        state["pluginAgentModelSelectionOverrides"],
        serde_json::json!({"plugin:demo:reviewer": {"providerId": "p", "modelId": "m"}}),
        "只留 plugin: 前缀的合法条目"
    );
    assert_eq!(
        state["disabledAgentIds"],
        serde_json::json!(["b", "a"]),
        "只留非空字符串并 trim（顺序保持原样，去重排序由 setEnabled 负责）"
    );
    // 旧字段与未知字段都被 `...parsed` 保留（本片不做 legacy 迁移，见 spec 登记差异）。
    assert_eq!(
        state["builtInModelOverrides"]["general-purpose"]["providerId"],
        serde_json::json!("legacy")
    );
    assert_eq!(state["futureField"]["keep"], serde_json::json!(true));
}
