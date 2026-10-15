//! 完整性判定单测（期望来自 zod complete schema 的语义；与 TS 的差分在 `rustHostProvider.test.ts`）。

use serde_json::{Value, json};

use super::*;

fn provider() -> Value {
    json!({
        "group":"standard-personal",
        "access":{"type":"api-key","apiKey":"sk-1"},
        "api":{"type":"openai-responses","baseUrl":"https://api.example.com/v1"},
        "builtinModelIds":["a"]
    })
}

fn model() -> Value {
    json!({
        "enabled":true,
        "properties":{
            "requiresMfjsToolSchema":false,"contextWindow":8192,
            "inputFormat":{"supportsText":true,"supportsImage":false,"supportsVideo":false,"supportsAudio":false,"supportsPdf":false},
            "outputFormat":{"supportsText":true},
            "supportsToolCall":true,"supportsJsonSchemaOutput":false,"supportsNativeWebSearch":false,"supportsMidConversationSystem":false
        },
        "optionSpecs":{
            "reasoningLevel":{"values":["low","high"],"map":"{'effort': reasoningLevel}"},
            "maxOutputTokens":{"max":4096,"map":"{'max_tokens': maxOutputTokens}"}
        }
    })
}

#[test]
fn provider_requires_group_access_and_api() {
    assert!(provider_complete(&provider()));
    for (key, mutate) in [
        ("group", json!(null)),
        ("access", json!({"type":"api-key","apiKey":"   "})),
        (
            "api",
            json!({"type":"openai-responses","baseUrl":"not a url"}),
        ),
        ("api", json!({"type":"nope","baseUrl":"https://x"})),
        ("visibility", json!("secret")),
        ("builtinModelIds", json!(["a", ""])),
    ] {
        let mut config = provider();
        config[key] = mutate;
        assert!(!provider_complete(&config), "{key} 应不合格");
    }
    let mut extra = provider();
    extra["surprise"] = json!(1);
    assert!(!provider_complete(&extra), "strict：多余键不合格");
}

#[test]
fn zhipu_account_access_is_a_complete_alternative() {
    let mut config = provider();
    config["access"] =
        json!({"type":"zhipu-account","accountType":"zai","mode":"start-plan","entitled":false});
    assert!(provider_complete(&config));
    config["access"]["mode"] = json!("free");
    assert!(!provider_complete(&config));
}

#[test]
fn model_requires_every_leaf() {
    assert!(model_complete(&model()));
    let mut no_window = model();
    no_window["properties"]
        .as_object_mut()
        .unwrap()
        .remove("contextWindow");
    assert!(!model_complete(&no_window));
    let mut zero = model();
    zero["properties"]["contextWindow"] = json!(0);
    assert!(!model_complete(&zero));
    let mut duplicate = model();
    duplicate["optionSpecs"]["reasoningLevel"]["values"] = json!(["low", "low"]);
    assert!(!model_complete(&duplicate));
    let mut blank = model();
    blank["optionSpecs"]["reasoningLevel"]["values"] = json!(["low", "  "]);
    assert!(!model_complete(&blank));
    let mut bad_map = model();
    bad_map["optionSpecs"]["maxOutputTokens"]["map"] = json!("{'max_tokens': ");
    assert!(!model_complete(&bad_map));
    let mut fractional = model();
    fractional["optionSpecs"]["maxOutputTokens"]["max"] = json!(1.5);
    assert!(!model_complete(&fractional));
}
