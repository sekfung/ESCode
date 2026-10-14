//! 已保存工作流的工具面（模型内容 / display / roots）与 TS oracle 语料逐条比对。
//! 存储面语料在 `saved_workflows_tests.rs`。
use super::saved_workflows as store;
use super::saved_workflows_tests::text;
use escode_cli_domain::json_order::Json;

const TOOL_CORPUS: &str = include_str!("../tests/fixtures/saved_workflow_tool_corpus.json");

fn tool_corpus() -> Json {
    Json::parse(TOOL_CORPUS).unwrap()
}

/// 语料的工具输出（`ListSavedWorkflowsOutput`）还原成存储层结构：路径只是字符串，解析/枚举本身
/// 已由存储语料覆盖。
fn listed_from(output: &Json) -> store::Listed {
    let entries = output
        .get("workflows")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .map(|entry| store::Entry {
            name: text(entry.get("name").unwrap()).to_owned(),
            description: text(entry.get("description").unwrap()).to_owned(),
            when_to_use: entry
                .get("whenToUse")
                .and_then(Json::as_str)
                .map(str::to_owned),
            args: entry.get("args").cloned(),
            scope: store::Scope::parse(text(entry.get("scope").unwrap())).unwrap(),
            path: std::path::PathBuf::from(text(entry.get("path").unwrap())),
        })
        .collect();
    let invalid = output
        .get("invalid")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .map(|invalid| store::Invalid {
            path: std::path::PathBuf::from(text(invalid.get("path").unwrap())),
            kind: store::InvalidKind::ParseError,
            reason: text(invalid.get("reason").unwrap()).to_owned(),
        })
        .collect();
    store::Listed { entries, invalid }
}

#[test]
fn tool_model_content_and_display_match_ts() {
    for case in tool_corpus().as_array().unwrap() {
        let listed = listed_from(case.get("output").unwrap());
        // 模型面：格式化的容器逐字比对（24 KiB 预算由 executor 施加，见下一个用例）。
        assert_eq!(
            store::format_model_content(&listed),
            text(case.get("modelContent").unwrap()),
            "{case:?}"
        );
        // 行级 display：键序无关，按 JSON 值比对（`truncated` 与 2 KiB 截断也在语料里）。
        assert_eq!(
            store::to_value(&store::display(&listed)),
            serde_json::from_str::<serde_json::Value>(&case.get("display").unwrap().compact())
                .unwrap(),
            "{case:?}"
        );
    }
}

#[test]
fn tool_model_content_respects_the_result_budget() {
    for case in tool_corpus().as_array().unwrap() {
        let listed = listed_from(case.get("output").unwrap());
        let content = store::model_content(&listed);
        assert!(content.len() <= 24_000, "{case:?}");
        let expected = if text(case.get("modelContent").unwrap()).len() <= 24_000 {
            text(case.get("modelContent").unwrap()).to_owned()
        } else {
            // TS `fitContentWithSuffix`：头部保留 `24000 - 后缀字节` 后再接后缀。
            let suffix = format!(
                "\n\n[Tool output truncated by resultBudget: originalBytes={}, maxModelBytes=24000, strategy=truncate]",
                text(case.get("modelContent").unwrap()).len()
            );
            let body = text(case.get("modelContent").unwrap());
            let mut end = 24_000 - suffix.len();
            while !body.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}{suffix}", &body[..end])
        };
        assert_eq!(content, expected, "{case:?}");
    }
}

#[test]
fn roots_use_platform_separators_like_node_path_join() {
    // TS `join(cwd, ".escode/workflows")` 会把常量里的 `/` 拆成两级；`Path::join` 不会，
    // Windows 上会留下 `\.escode/workflows\` 的混合分隔符（App 差分的 display 路径抓到过）。
    let cwd = std::path::Path::new("C:\\repo");
    let home = std::path::Path::new("C:\\home");
    let roots = store::roots(cwd, home);
    assert_eq!(roots[0].dir, cwd.join(".escode").join("workflows"));
    assert_eq!(roots[1].dir, home.join(".escode").join("workflows"));
}
