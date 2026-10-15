//! Provider / Model 配置的 Overlay 语义（TS `ConfigOverlay`：`overlayValue` / `overlayConfig`）。
//!
//! JSON 里「键缺失」= TS 的 `undefined`（继承 base），「键为 null」= 显式 `null`（整体替换）。
//! 嵌套配置对象（`api`、`properties`、`inputFormat` …）在两边都是对象时递归叠加，其余值（含数组、`headers`、
//! `logo`）整体替换；`access` 的 `type` 不同则整体替换（判别联合不能继承另一种认证方式的字段）。

use serde_json::{Map, Value};

/// 会递归叠加的嵌套键（按所在层级区分，避免同名叶子被误当作嵌套配置）。
#[derive(Clone, Copy)]
pub enum Kind {
    Provider,
    Model,
}

fn nested_keys(kind: Kind, parent: &str) -> &'static [&'static str] {
    match (kind, parent) {
        (Kind::Provider, "") => &["access", "api"],
        (Kind::Model, "") => &["properties", "optionSpecs"],
        (Kind::Model, "properties") => &["inputFormat", "outputFormat"],
        (Kind::Model, "optionSpecs") => &["reasoningLevel", "maxOutputTokens"],
        _ => &[],
    }
}

fn overlay_at(
    kind: Kind,
    parent: &str,
    base: &Map<String, Value>,
    next: &Map<String, Value>,
) -> Map<String, Value> {
    let mut result = base.clone();
    for (key, value) in next {
        let nested = nested_keys(kind, parent).contains(&key.as_str());
        match (nested, base.get(key), value) {
            (true, Some(Value::Object(current)), Value::Object(incoming)) => {
                // access 的 type 不同：整体替换。
                let replace = matches!(kind, Kind::Provider)
                    && key == "access"
                    && current.get("type") != incoming.get("type");
                if replace {
                    result.insert(key.clone(), value.clone());
                } else {
                    result.insert(
                        key.clone(),
                        Value::Object(overlay_at(kind, key, current, incoming)),
                    );
                }
            }
            _ => {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    result
}

/// `base.overlay(next)`：两者都是配置对象。
pub fn overlay(kind: Kind, base: &Value, next: &Value) -> Value {
    let empty = Map::new();
    Value::Object(overlay_at(
        kind,
        "",
        base.as_object().unwrap_or(&empty),
        next.as_object().unwrap_or(&empty),
    ))
}

/// TS `ProviderConfig.withoutGroup()`：丢弃 `group`（其余字段不变）。
pub fn without_group(config: &Value) -> Value {
    let mut object = config.as_object().cloned().unwrap_or_default();
    object.remove("group");
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn missing_inherits_null_replaces_and_nested_objects_merge() {
        let base = json!({"group":"zai-family","api":{"type":"anthropic-messages","baseUrl":"https://a","headers":{"x":"1"}},"visibility":"visible"});
        let next = json!({"api":{"baseUrl":"https://b","headers":{"y":"2"}},"visibility":null});
        let merged = overlay(Kind::Provider, &base, &next);
        assert_eq!(merged["group"], "zai-family");
        assert_eq!(
            merged["api"],
            json!({"type":"anthropic-messages","baseUrl":"https://b","headers":{"y":"2"}})
        );
        assert_eq!(merged["visibility"], Value::Null, "null 是显式值");
        let replaced = overlay(
            Kind::Provider,
            &json!({"api":{"type":"x"}}),
            &json!({"api":null}),
        );
        assert_eq!(replaced["api"], Value::Null);
    }

    #[test]
    fn access_with_a_different_type_is_replaced_not_merged() {
        let base = json!({"access":{"type":"api-key","apiKey":"secret","apiKeyManagementUrl":"https://k"}});
        let same = overlay(
            Kind::Provider,
            &base,
            &json!({"access":{"type":"api-key","apiKey":"new"}}),
        );
        assert_eq!(
            same["access"],
            json!({"type":"api-key","apiKey":"new","apiKeyManagementUrl":"https://k"})
        );
        let other = overlay(
            Kind::Provider,
            &base,
            &json!({"access":{"type":"zhipu-account","entitled":false}}),
        );
        assert_eq!(
            other["access"],
            json!({"type":"zhipu-account","entitled":false})
        );
    }

    #[test]
    fn model_config_merges_two_levels_and_replaces_arrays() {
        let base = json!({"enabled":true,"properties":{"contextWindow":1,"inputFormat":{"supportsText":true,"supportsImage":false}},
            "optionSpecs":{"reasoningLevel":{"values":["a","b"],"map":"m1"}}});
        let next = json!({"properties":{"inputFormat":{"supportsImage":true}},"optionSpecs":{"reasoningLevel":{"values":["c"]}}});
        let merged = overlay(Kind::Model, &base, &next);
        assert_eq!(
            merged["properties"]["inputFormat"],
            json!({"supportsText":true,"supportsImage":true})
        );
        assert_eq!(
            merged["optionSpecs"]["reasoningLevel"],
            json!({"values":["c"],"map":"m1"})
        );
        assert_eq!(merged["properties"]["contextWindow"], 1);
    }

    #[test]
    fn without_group_drops_only_group() {
        let config = json!({"group":"standard-personal","visibility":"hidden"});
        assert_eq!(without_group(&config), json!({"visibility":"hidden"}));
    }
}
