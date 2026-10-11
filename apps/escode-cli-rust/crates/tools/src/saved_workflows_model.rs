//! ListSavedWorkflows 的模型面 / 输出 / display（TS list-saved-workflows.ts 的格式化部分）。
#[allow(unused_imports)]
use super::saved_workflows::*;

use escode_cli_domain::json_order::Json;

/// 列表里的一行 → TS `SavedWorkflowEntry` 的 JSON（键序同 TS 的对象字面量）。
pub fn entry_json(entry: &Entry) -> Json {
    let mut value = Json::object();
    value.set("name", Json::str(&entry.name));
    value.set("description", Json::str(&entry.description));
    if let Some(when) = &entry.when_to_use {
        value.set("whenToUse", Json::str(when));
    }
    if let Some(args) = &entry.args {
        value.set("args", args.clone());
    }
    value.set("scope", Json::str(entry.scope.as_str()));
    value.set("path", Json::str(entry.path.to_string_lossy().into_owned()));
    value
}

/// 工具模型通道预算（TS `LIST_SAVED_WORKFLOWS_MODEL_BYTES`）。
pub(super) const MODEL_BYTES: usize = 24_000;

/// TS `WORKFLOW_OBSERVATION_DISPLAY_MAX_META_CHARS`：display 侧自由文本的字节上限。
pub(super) const DISPLAY_META_BYTES: usize = 2_048;

/// 工具结构化输出（TS `ListSavedWorkflowsOutput`）：`invalid` 为空时缺席。
pub fn output(listed: &Listed) -> Json {
    let mut value = Json::object();
    value.set(
        "workflows",
        Json::Array(listed.entries.iter().map(entry_json).collect()),
    );
    if !listed.invalid.is_empty() {
        value.set(
            "invalid",
            Json::Array(
                listed
                    .invalid
                    .iter()
                    .map(|invalid| {
                        let mut entry = Json::object();
                        entry.set(
                            "path",
                            Json::str(invalid.path.to_string_lossy().into_owned()),
                        );
                        entry.set("reason", Json::str(&invalid.reason));
                        entry
                    })
                    .collect(),
            ),
        );
    }
    value
}

/// 模型面（TS `formatListSavedWorkflowsModelContent`）：一个 XML-ish 容器 + 一 workflow 一块。
///
/// 与 ListWorkflowRuns 的单行属性式刻意不同：保存的定义带着描述、使用时机与参数表，那些正是模型
/// 用来**选**工作流的依据；条数也低得多，撑得起每条几行。
pub fn format_model_content(listed: &Listed) -> String {
    if listed.entries.is_empty() && listed.invalid.is_empty() {
        // 「这个项目没存过 workflow」必须说成一句话：空容器容易被读成「工具没答上来」。
        return format!(
            "<saved_workflows count=\"0\">\nNo workflows are saved in this project yet. Saved definitions live in {PROJECT_DIR}/.\n</saved_workflows>"
        );
    }
    let mut lines = vec![format!(
        "<saved_workflows count=\"{}\">",
        listed.entries.len()
    )];
    for entry in &listed.entries {
        lines.push(format!(
            "<workflow name=\"{}\" scope=\"{}\">",
            escape_attribute(&entry.name),
            entry.scope.as_str()
        ));
        lines.push(format!("  {}", entry.description));
        if let Some(when) = &entry.when_to_use {
            lines.push(format!("  When to use: {when}"));
        }
        if let Some(Json::Object(args)) = &entry.args {
            for (key, spec) in args {
                let mut notes = vec![
                    spec.get("type")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ];
                if spec.get("required") == Some(&Json::Bool(true)) {
                    notes.push("required".to_owned());
                }
                if let Some(default) = spec.get("default") {
                    notes.push(format!("default {}", default.compact()));
                }
                let description = spec
                    .get("description")
                    .and_then(Json::as_str)
                    .map(|text| format!(" — {text}"))
                    .unwrap_or_default();
                lines.push(format!("  arg {key} ({}){description}", notes.join(", ")));
            }
        }
        lines.push("</workflow>".to_owned());
    }
    for invalid in &listed.invalid {
        lines.push(format!(
            "<invalid path=\"{}\">{}</invalid>",
            escape_attribute(&invalid.path.to_string_lossy()),
            invalid.reason
        ));
    }
    lines.push("</saved_workflows>".to_owned());
    lines.join("\n")
}

/// 工具通道的模型内容：格式化结果再按 `resultBudget` 截断（TS `LIST_SAVED_WORKFLOWS_MODEL_BYTES`）。
pub fn model_content(listed: &Listed) -> String {
    escode_cli_domain::tool_display::truncate_model_content(
        format_model_content(listed),
        MODEL_BYTES,
    )
}

/// 行级 display（TS `createSavedWorkflowListDisplay`，kind `saved_workflow_list`）：
/// 元文本按 display 通道自己的 2 KiB 上限截断（工具通道的 24 KiB 兜不住 display）。
pub fn display(listed: &Listed) -> Json {
    let mut truncated = false;
    let mut bound = |value: &str| {
        let (text, cut) =
            escode_cli_domain::tool_display::bound_display_text(value, DISPLAY_META_BYTES);
        truncated |= cut;
        text
    };
    let workflows = listed
        .entries
        .iter()
        .map(|entry| {
            let mut value = Json::object();
            value.set("name", Json::str(&entry.name));
            value.set("description", Json::str(bound(&entry.description)));
            if let Some(when) = &entry.when_to_use {
                let when = bound(when);
                value.set("whenToUse", Json::str(when));
            }
            value.set("scope", Json::str(entry.scope.as_str()));
            value.set("path", Json::str(entry.path.to_string_lossy().into_owned()));
            // args 只保留名字：声明细节（类型/描述/默认值）归保存确认窗，列表卡不重复。
            let names = match &entry.args {
                Some(Json::Object(args)) => args.iter().map(|(key, _)| Json::str(key)).collect(),
                _ => Vec::new(),
            };
            value.set("argNames", Json::Array(names));
            value
        })
        .collect::<Vec<_>>();
    let invalid = listed
        .invalid
        .iter()
        .map(|invalid| {
            let mut value = Json::object();
            value.set(
                "path",
                Json::str(invalid.path.to_string_lossy().into_owned()),
            );
            value.set("reason", Json::str(bound(&invalid.reason)));
            value
        })
        .collect::<Vec<_>>();
    let mut value = Json::object();
    value.set("kind", Json::str("saved_workflow_list"));
    value.set("workflows", Json::Array(workflows));
    if !invalid.is_empty() {
        value.set("invalid", Json::Array(invalid));
    }
    if truncated {
        value.set("truncated", Json::Bool(true));
    }
    value
}

/// 名字与路径进属性位：两者都可能带引号（路径尤其），不转义会造出畸形标签。
pub(super) fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// 保序 JSON → `serde_json::Value`（工具结果载荷用；键序已经按 TS 对象字面量排好）。
pub fn to_value(value: &Json) -> serde_json::Value {
    serde_json::from_str(&value.compact()).unwrap_or(serde_json::Value::Null)
}
