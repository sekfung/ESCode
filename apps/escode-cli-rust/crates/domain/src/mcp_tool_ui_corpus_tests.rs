//! 与 TS oracle 语料逐条比对（scripts/generate-escode-cli-rust-mcp-tool-ui-corpus.mjs）。
//! 期望值全部由 TS `normalizeMcpToolUiMeta` / `createMcpToolDisplay` 生成；超预算载荷按语料的
//! `repeat` 计划在两边用同一份构造重建，避免把填充字符串写进仓库。
use super::{display_ui, normalize_meta, plugin_id_for_server};
use serde_json::{Map, Value, json};

fn corpus() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/mcp_tool_ui_corpus.json")).unwrap()
}

fn build_repeats(specs: &Value) -> Value {
    let mut output = Map::new();
    for spec in specs.as_array().unwrap() {
        let field = spec["field"].as_str().unwrap();
        let text = spec["char"]
            .as_str()
            .unwrap()
            .repeat(spec["times"].as_u64().unwrap() as usize);
        let value = if field == "content" {
            json!([{ "text": text, "type": "text" }])
        } else {
            Value::String(text)
        };
        output.insert(field.to_owned(), value);
    }
    Value::Object(output)
}

#[test]
fn descriptors_match_ts() {
    for case in corpus()["meta"].as_array().unwrap() {
        let label = case["label"].as_str().unwrap();
        let actual = normalize_meta(&case["meta"]);
        let expected = &case["descriptor"];
        if expected.is_null() {
            assert!(actual.is_none(), "{label}");
        } else {
            assert_eq!(actual.as_ref(), Some(expected), "{label}");
        }
    }
}

#[test]
fn display_ui_matches_ts() {
    for case in corpus()["displayUi"].as_array().unwrap() {
        let label = case["label"].as_str().unwrap();
        let plugin_id = case["pluginId"].as_str().unwrap();
        if case["pluginIdSource"] == "server" {
            assert_eq!(
                plugin_id_for_server(case["serverName"].as_str().unwrap()),
                plugin_id,
                "{label}"
            );
        }
        let output = if case.get("repeat").is_some() {
            Some(build_repeats(&case["repeat"]))
        } else {
            case["output"].as_object().cloned().map(Value::Object)
        };
        let actual = normalize_meta(&case["meta"])
            .as_ref()
            .and_then(|descriptor| display_ui(descriptor, plugin_id, output.as_ref()));
        let expected = if case["display"].is_null() || case["display"]["ui"].is_null() {
            None
        } else {
            Some(case["display"]["ui"].clone())
        };
        assert_eq!(actual, expected, "{label}");
    }
}
