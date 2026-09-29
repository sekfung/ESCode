//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-model-catalog-corpus.mjs）：
//! 模型面文本逐字、行级 display 值相等（含 2 KiB 元文本截断与 100 行封顶）。
use super::*;

const CORPUS: &str = include_str!("../tests/fixtures/model_catalog_corpus.json");

fn corpus() -> Json {
    Json::parse(CORPUS).unwrap()
}

fn text(value: &Json) -> &str {
    value.as_str().expect("语料里的字符串")
}

/// 语料的工具输出 → 条目（`current` 单独取）。
fn entries_of(output: &Json) -> Vec<Entry> {
    output
        .get("models")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .map(|model| Entry {
            provider_id: text(model.get("providerId").unwrap()).to_owned(),
            model_id: text(model.get("modelId").unwrap()).to_owned(),
            provider_label: model.get("providerLabel").and_then(Json::as_str).map(str::to_owned),
            reasoning_levels: model
                .get("reasoningLevels")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .map(|level| text(level).to_owned())
                .collect(),
            default_reasoning_level: model
                .get("defaultReasoningLevel")
                .and_then(Json::as_str)
                .map(str::to_owned),
            context_window: model.get("contextWindow").and_then(|value| match value {
                Json::Number(number) => number.as_i64(),
                _ => None,
            }),
            disabled_reason: model.get("disabledReason").and_then(Json::as_str).map(str::to_owned),
        })
        .collect()
}

#[test]
fn model_content_and_display_match_ts() {
    for case in corpus().as_array().unwrap() {
        let expected_output = case.get("output").unwrap();
        let current = expected_output.get("current").and_then(Json::as_str);
        let entries = entries_of(expected_output);
        // 工具输出形状本身也要对齐（`current` 缺席即会话选择指向已被删掉的 provider）。
        assert_eq!(
            output(current, &entries).compact(),
            expected_output.compact(),
            "{case:?}"
        );
        assert_eq!(
            format_model_content(current, &entries),
            text(case.get("modelContent").unwrap()),
            "{case:?}"
        );
        assert_eq!(
            to_value(&display(current, &entries)),
            serde_json::from_str::<serde_json::Value>(
                &case.get("display").unwrap().compact()
            )
            .unwrap(),
            "{case:?}"
        );
    }
}

#[test]
fn model_content_respects_the_result_budget() {
    for case in corpus().as_array().unwrap() {
        let expected_output = case.get("output").unwrap();
        let current = expected_output.get("current").and_then(Json::as_str);
        let entries = entries_of(expected_output);
        let content = model_content(current, &entries);
        assert!(content.len() <= 24_000, "{case:?}");
        if text(case.get("modelContent").unwrap()).len() <= 24_000 {
            assert_eq!(content, text(case.get("modelContent").unwrap()), "{case:?}");
        } else {
            assert!(content.contains("truncated by resultBudget"), "{case:?}");
        }
    }
}
