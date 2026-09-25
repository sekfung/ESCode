//! 模型可见工具定义：描述、分支与顺序对齐 Node（docs/specs/rust-tool-surface.md）。

use serde_json::{Value, json};

pub(super) fn definitions() -> Vec<Value> {
    let schemas: Value =
        serde_json::from_str(include_str!("tool_schemas.json")).expect("validated tool schemas");
    // 模型可见工具面与 Node 对齐（docs/specs/rust-tool-surface.md）：描述取 TS provider 描述；
    // Rust 不支持会话级工具白名单，Bash 恒可用，因此与 TS 默认一致走 embedded search 分支，
    // 不暴露 Glob/Grep（仍可执行，只是不再提供给模型），Bash/EnterPlanMode 取该分支的描述。
    let surface = tool_surface();
    let mut definitions: Vec<Value> = ["Read", "Write", "Edit", "TaskOutput", "TaskStop", "WebFetch"]
        .into_iter()
        .map(|name| json!({"type":"function","function":{"name":name,"description":surface["descriptions"][name],"parameters":schemas[name]}}))
        .collect();
    definitions.push(json!({"type":"function","function":{"name":"Bash","description":surface["Bash"]["embedded"],"parameters":schemas["Bash"]}}));
    let description: String = serde_json::from_str(include_str!("skill_description.json"))
        .expect("validated Skill description");
    definitions.push(json!({"type":"function","function":{"name":"Skill","description":description,"parameters":schemas["Skill"]}}));
    let description: String = serde_json::from_str(include_str!("question_description.json"))
        .expect("validated question description");
    definitions.push(json!({"type":"function","function":{"name":"AskUserQuestion","description":description,"parameters":schemas["AskUserQuestion"]}}));
    let descriptions: Value = serde_json::from_str(include_str!("todo_descriptions.json"))
        .expect("validated todo descriptions");
    for name in ["TodoRead", "TodoWrite"] {
        definitions.push(json!({"type":"function","function":{"name":name,"description":descriptions[name],"parameters":schemas[name]}}));
    }
    let descriptions: Value =
        serde_json::from_str(include_str!("agent_descriptions.json")).expect("agent descriptions");
    for name in ["Agent", "SendMessage"] {
        definitions.push(json!({"type":"function","function":{"name":name,"description":descriptions[name],"parameters":schemas[name]}}));
    }
    definitions.extend(super::plan_tools::definitions(
        &schemas,
        &surface["EnterPlanMode"]["embedded"],
    ));
    order_like_provider(definitions, surface)
}

fn tool_surface() -> &'static Value {
    static SURFACE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SURFACE.get_or_init(|| {
        serde_json::from_str(include_str!("tool_surface.json")).expect("generated tool surface")
    })
}

/// TS `orderProviderVisibleToolContracts`：参考集合内按名称排序（顺序由生成资产给出），其余保持原顺序排在后面。
fn order_like_provider(definitions: Vec<Value>, surface: &Value) -> Vec<Value> {
    let order: Vec<&str> = surface["providerOrder"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let rank = |definition: &Value| {
        let name = definition["function"]["name"].as_str().unwrap_or_default();
        order.iter().position(|candidate| *candidate == name)
    };
    let (mut reference, local): (Vec<Value>, Vec<Value>) =
        definitions.into_iter().partition(|d| rank(d).is_some());
    reference.sort_by_key(|d| rank(d));
    reference.extend(local);
    reference
}
