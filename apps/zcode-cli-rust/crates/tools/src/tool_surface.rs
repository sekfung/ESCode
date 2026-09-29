//! 模型可见工具定义：描述、分支与顺序对齐 Node（docs/specs/rust-tool-surface.md）。

use serde_json::{Value, json};

pub(super) fn definitions() -> Vec<Value> {
    let schemas: Value =
        serde_json::from_str(include_str!("tool_schemas.json")).expect("validated tool schemas");
    // 内置 schema 按声明顺序登记：发给模型的定义与入参校验的问题顺序同 Node（docs/specs/rust-tool-schema-order.md）。
    static ORDERED: std::sync::Once = std::sync::Once::new();
    ORDERED.call_once(|| {
        if let Some(crate::domain::json_order::Json::Object(entries)) =
            crate::domain::json_order::Json::parse(include_str!("tool_schemas.json"))
        {
            entries.iter().for_each(|(_, schema)| crate::domain::schema_order::remember(schema));
        }
    });
    // 模型可见工具面与 Node 对齐（docs/specs/rust-tool-surface.md）：描述取 TS provider 描述；
    // Rust 不支持会话级工具白名单，Bash 恒可用，因此与 TS 默认一致走 embedded search 分支，
    // 不暴露 Glob/Grep（仍可执行，只是不再提供给模型），Bash/EnterPlanMode 取该分支的描述。
    let surface = tool_surface();
    let mut definitions: Vec<Value> = ["Read", "Write", "Edit", "TaskOutput", "TaskStop", "WebFetch", "CronCreate", "CronList", "CronUpdate", "CronDelete"]
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
    // OffPeak 在 TS 注册顺序中位于 SendMessage 之前；是否可见由会话工具面开关决定（docs/specs/rust-offpeak.md）。
    for name in ["OffPeakCreate", "OffPeakList"] {
        definitions.push(json!({"type":"function","function":{"name":name,"description":surface["descriptions"][name],"parameters":schemas[name]}}));
    }
    let descriptions: Value =
        serde_json::from_str(include_str!("agent_descriptions.json")).expect("agent descriptions");
    for name in ["Agent", "SendMessage"] {
        definitions.push(json!({"type":"function","function":{"name":name,"description":descriptions[name],"parameters":schemas[name]}}));
    }
    // WebSearch 描述带当前月份，按 TS 每次构造时重新生成；可见性由会话按模型能力过滤。
    let (year, month) = zcode_cli_host::local_year_month();
    definitions.push(json!({"type":"function","function":{"name":"WebSearch","description":crate::domain::web_search::description(year, month),"parameters":schemas["WebSearch"]}}));
    // 非参考集合的工具按 TS 注册顺序排列：ReadSessionContext 在 SendMessage 之后。
    definitions.push(json!({"type":"function","function":{"name":"ReadSessionContext","description":surface["descriptions"]["ReadSessionContext"],"parameters":schemas["ReadSessionContext"]}}));
    // 已保存工作流清单（docs/specs/rust-dynamic-workflow.md 第 2 期）：只读、无 gate，是否可见由
    // 动态工作流灰度门决定（保留给各期实现顺序里的最后一个工作流工具之前）。
    definitions.push(json!({"type":"function","function":{"name":"ListSavedWorkflows","description":surface["descriptions"]["ListSavedWorkflows"],"parameters":schemas["ListSavedWorkflows"]}}));
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

/// 模型支持 PDF 时 Read 改用 TS resolveReadInputSchema / resolveReadProviderDescription 的结果。
pub(super) fn apply_pdf_read(definitions: &mut [Value]) {
    let variant = &tool_surface()["readPdf"];
    if let Some(read) = definitions
        .iter_mut()
        .find(|d| d["function"]["name"] == "Read")
    {
        read["function"]["description"] = variant["description"].clone();
        read["function"]["parameters"] = variant["parameters"].clone();
    }
}
