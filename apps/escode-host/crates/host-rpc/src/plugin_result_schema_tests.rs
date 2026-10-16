use serde_json::{Value, json};

use super::*;
use crate::plugin_result_tables::{PLUGINS_LIST_RESULT, PLUGINS_OVERVIEW_RESULT, schema_for};

/// TS oracle 语料（`scripts/generate-escode-host-plugin-schema-fixtures.mjs`）：真实 zod 对样本及其变异求 `safeParse().success`。
const GOLDEN: &str = include_str!("plugin_schema_golden.json");

fn node_at<'a>(root: &'a mut Value, path: &[Value]) -> &'a mut Value {
    let mut cursor = root;
    for key in path {
        cursor = match key {
            Value::String(name) => &mut cursor[name.as_str()],
            Value::Number(index) => &mut cursor[index.as_u64().expect("index") as usize],
            other => panic!("bad path segment {other}"),
        };
    }
    cursor
}

fn apply(base: &Value, op: u64, path: &[Value], replacement: &Value) -> Value {
    let mut value = base.clone();
    match op {
        0 => {}
        1 => {
            node_at(&mut value, path)
                .as_object_mut()
                .expect("object")
                .insert("__extra".into(), json!(1));
        }
        2 => {
            let (last, parent) = path.split_last().expect("non-empty path");
            node_at(&mut value, parent)
                .as_object_mut()
                .expect("object")
                .remove(last.as_str().expect("key"));
        }
        3 => *node_at(&mut value, path) = replacement.clone(),
        other => panic!("unknown op {other}"),
    }
    value
}

#[test]
fn verdicts_match_the_real_zod_schemas_on_the_whole_corpus() {
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden parses");
    assert_eq!(golden["fixtureVersion"], 1);
    let replacements = golden["replacements"].as_array().expect("replacements");
    let bases = golden["bases"].as_array().expect("bases");
    let cases = golden["cases"].as_array().expect("cases");
    assert!(cases.len() > 5000, "语料规模异常: {}", cases.len());
    let mut mismatches = Vec::new();
    for case in cases {
        let [base, op, path, replacement, ok] = case.as_array().expect("case").as_slice() else {
            panic!("malformed case {case}");
        };
        let base = &bases[base.as_u64().expect("base") as usize];
        let schema = schema_for(base["schema"].as_str().expect("schema")).expect("known schema");
        let value = apply(
            &base["value"],
            op.as_u64().expect("op"),
            path.as_array().expect("path"),
            &replacements[replacement.as_u64().expect("replacement") as usize],
        );
        let verdict = validate(schema, &value);
        if verdict.is_ok() != ok.as_bool().expect("ok") {
            mismatches.push(format!(
                "TS={} Rust={:?} :: {}",
                ok,
                verdict.err(),
                serde_json::to_string(&value)
                    .unwrap_or_default()
                    .chars()
                    .take(160)
                    .collect::<String>()
            ));
            if mismatches.len() >= 5 {
                break;
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "与 TS 判定不一致:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn json_floats_with_no_fraction_count_as_integers_like_js_numbers() {
    // JSON.stringify 无法表达 `1.0`，语料覆盖不到；JS 里 `1.0 === 1`，zod `.int()` 接受。
    let value: Value =
        serde_json::from_str(r#"{"marketplaces":[{"id":"a","name":"a","source":{},"pluginCount":3.0}],"availablePlugins":[],"installedPlugins":[],"restorableBuiltins":[],"diagnostics":[],"capability":{"supported":true}}"#)
            .unwrap();
    assert!(validate(&PLUGINS_OVERVIEW_RESULT, &value).is_ok());
    let fractional = serde_json::from_str::<Value>(
        r#"{"marketplaces":[{"id":"a","name":"a","source":{},"pluginCount":3.5}],"availablePlugins":[],"installedPlugins":[],"restorableBuiltins":[],"diagnostics":[],"capability":{"supported":true}}"#,
    )
    .unwrap();
    assert!(validate(&PLUGINS_OVERVIEW_RESULT, &fractional).is_err());
}

#[test]
fn errors_name_the_method_free_path_and_reason() {
    let error = validate(
        &PLUGINS_LIST_RESULT,
        &json!({"plugins": [{"id": ""}], "diagnostics": []}),
    )
    .unwrap_err();
    assert!(error.starts_with("result.plugins.0.id: "), "{error}");
}

#[test]
fn every_management_method_has_a_result_schema() {
    use crate::plugin_params::SPECS;
    let missing: Vec<&str> = SPECS
        .iter()
        .filter(|spec| schema_for(spec.runtime).is_none())
        .map(|spec| spec.runtime)
        .collect();
    assert!(
        missing.is_empty(),
        "新增管理面方法必须同时登记结果 schema（并在生成器 SAMPLES 里加样本）: {missing:?}"
    );
}

#[test]
fn methods_sharing_a_ts_schema_share_the_table() {
    let same = |a: &str, b: &str| std::ptr::eq(schema_for(a).unwrap(), schema_for(b).unwrap());
    assert!(same("plugins/install", "plugins/update"));
    assert!(same("plugins/configure", "plugins/resetConfig"));
    assert!(same(
        "plugins/marketplace/add",
        "plugins/marketplace/remove"
    ));
    assert!(!same("plugins/install", "plugins/uninstall"));
}

#[test]
fn defaults_are_filled_exactly_like_zod_parse() {
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden parses");
    let normalizations = golden["normalizations"].as_array().expect("normalizations");
    assert!(normalizations.len() >= 3, "语料缺少归一化样本");
    for case in normalizations {
        let method = case["schema"].as_str().expect("schema");
        let schema = schema_for(method).expect("known schema");
        let mut value = case["input"].clone();
        validate(schema, &value).expect("input is accepted");
        crate::plugin_result_tables::normalize(method, &mut value);
        assert_eq!(value, case["output"], "{method}");
    }
}

#[test]
fn normalization_leaves_every_other_method_untouched() {
    let original = json!({"plugins": [{"id": "a"}], "diagnostics": []});
    let mut value = original.clone();
    crate::plugin_result_tables::normalize("plugins/list", &mut value);
    assert_eq!(value, original, "只有引用目录补默认值");
}

#[test]
fn a_null_subagent_list_is_rejected_not_defaulted() {
    // zod `.default` 只在 `undefined` 时生效；`null` 是类型错误。
    let value = json!({"authority": "session", "plugins": [{
        "pluginId": "a@mk", "name": "a", "marketplace": "mk", "enabled": true,
        "conflictingPluginIds": [], "skillQualifiedNames": [], "mcpServerNames": [],
        "subagentNames": null
    }]});
    let schema = schema_for("plugins/referenceCatalogWithCategory").unwrap();
    assert!(validate(schema, &value).is_err());
}
