//! `ListModels` 的目录条目与两个面（docs/specs/rust-dynamic-workflow.md 第 6 期前置），对齐 TS
//! `core/src/tool/handlers/list-models.ts`、`handlers/model-reference.ts` 的 `formatModelCatalogId`
//! 与 `executor/workflow-observation-display.ts` 的 `createListModelsDisplay`。
//!
//! 目录来源（Provider Registry → 条目）在 model crate；这里只做**纯投影**：条目 → 工具输出、
//! 模型面文本、行级 display。
use crate::json_order::Json;

/// 工具通道预算（TS `LIST_MODELS_MODEL_BYTES`）。
const MODEL_BYTES: usize = 24_000;
/// TS `WORKFLOW_OBSERVATION_DISPLAY_MAX_META_CHARS`：display 侧自由文本的字节上限。
const DISPLAY_META_BYTES: usize = 2_048;
/// TS `WORKFLOW_OBSERVATION_DISPLAY_MAX_MODELS`：目录卡一次最多画几行模型。
const DISPLAY_MAX_MODELS: usize = 100;

/// 一个目录条目（TS `ModelCatalogEntry`；`current` 是列举那一刻与会话选择的比对结果，单独传）。
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub provider_id: String,
    pub model_id: String,
    /// provider 的人类可读名；注册表没给名字时缺席（读侧退回 providerId）。
    pub provider_label: Option<String>,
    /// 档位表（原样复制注册表视图里的顺序）。
    pub reasoning_levels: Vec<String>,
    /// 默认档位 = 最后一档（与 GUI picker 同一条规则）；没有档位时缺席。
    pub default_reasoning_level: Option<String>,
    pub context_window: Option<i64>,
    /// 今天宿主没有这条事实的来源，恒缺席；将来补「配了但不可用」时填这里。
    pub disabled_reason: Option<String>,
}

impl Entry {
    /// TS `formatModelCatalogId`：`providerId/modelId`（不含档位）。
    pub fn id(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

fn number(value: i64) -> Json {
    Json::Number(value.into())
}

/// TS `ListModelsOutput`：`current` 缺席即会话选择指向一个已被删掉的 provider；`models` 恒在场。
pub fn output(current: Option<&str>, entries: &[Entry]) -> Json {
    let mut value = Json::object();
    if let Some(current) = current {
        value.set("current", Json::str(current));
    }
    value.set(
        "models",
        Json::Array(
            entries
                .iter()
                .map(|entry| {
                    let mut model = Json::object();
                    model.set("id", Json::str(entry.id()));
                    model.set("providerId", Json::str(&entry.provider_id));
                    model.set("modelId", Json::str(&entry.model_id));
                    if let Some(label) = &entry.provider_label {
                        model.set("providerLabel", Json::str(label));
                    }
                    // 没有档位的模型给空数组而不是缺席：读侧据此知道「接 `$` 是错的」。
                    model.set(
                        "reasoningLevels",
                        Json::Array(entry.reasoning_levels.iter().map(Json::str).collect()),
                    );
                    if let Some(level) = &entry.default_reasoning_level {
                        model.set("defaultReasoningLevel", Json::str(level));
                    }
                    if let Some(window) = entry.context_window {
                        model.set("contextWindow", number(window));
                    }
                    if let Some(reason) = &entry.disabled_reason {
                        model.set("disabledReason", Json::str(reason));
                    }
                    model
                })
                .collect(),
        ),
    );
    value
}

/// TS `formatListModelsModelContent`：一行一个模型；`current` 与 `disabled` 排在行尾且各用方括号
/// （它们是模型据以**排除**一行的两个标记）。
pub fn format_model_content(current: Option<&str>, entries: &[Entry]) -> String {
    if entries.is_empty() {
        // 「一个都没配」必须说成一句话：空容器容易被读成「工具没答上来」。
        return [
            "<models count=\"0\">",
            "No models are configured on this host. Omit `subagent_model`: the workflow's subagents run on the session model.",
            "</models>",
        ]
        .join("\n");
    }
    let lines = entries.iter().map(|entry| {
        let id = entry.id();
        let mut text = id.clone();
        if let Some(label) = &entry.provider_label {
            text.push_str(&format!(" — {label}"));
        }
        if !entry.reasoning_levels.is_empty() {
            let fallback = entry
                .default_reasoning_level
                .as_ref()
                .map(|level| format!(" (default {level})"))
                .unwrap_or_default();
            text.push_str(&format!("; levels: {}{fallback}", entry.reasoning_levels.join(",")));
        }
        if current == Some(id.as_str()) {
            text.push_str(" [current]");
        }
        if let Some(reason) = &entry.disabled_reason {
            text.push_str(&format!(" [disabled: {reason}]"));
        }
        text
    });
    [
        format!("<models count=\"{}\">", entries.len()),
        lines.collect::<Vec<_>>().join("\n"),
        "</models>".to_owned(),
    ]
    .join("\n")
}

/// 工具通道的模型内容：格式化结果再按 `resultBudget` 截断（同 ListSavedWorkflows）。
pub fn model_content(current: Option<&str>, entries: &[Entry]) -> String {
    crate::tool_display::truncate_model_content(format_model_content(current, entries), MODEL_BYTES)
}

/// TS `createListModelsDisplay`（kind `list_models`）：元文本按 display 通道的 2 KiB 上限截断，
/// 行数按 100 封顶（超出即 `truncated`）。
pub fn display(current: Option<&str>, entries: &[Entry]) -> Json {
    let mut truncated = false;
    let mut bound = |value: &str| {
        let (text, cut) = crate::tool_display::bound_display_text(value, DISPLAY_META_BYTES);
        truncated |= cut;
        text
    };
    let models = entries
        .iter()
        .take(DISPLAY_MAX_MODELS)
        .map(|entry| {
            let mut model = Json::object();
            model.set("id", Json::str(entry.id()));
            model.set("providerId", Json::str(&entry.provider_id));
            model.set("modelId", Json::str(&entry.model_id));
            if let Some(label) = &entry.provider_label {
                let label = bound(label);
                model.set("providerLabel", Json::str(label));
            }
            model.set(
                "reasoningLevels",
                Json::Array(entry.reasoning_levels.iter().map(Json::str).collect()),
            );
            if let Some(level) = &entry.default_reasoning_level {
                model.set("defaultReasoningLevel", Json::str(level));
            }
            if let Some(window) = entry.context_window {
                model.set("contextWindow", number(window));
            }
            if let Some(reason) = &entry.disabled_reason {
                let reason = bound(reason);
                model.set("disabledReason", Json::str(reason));
            }
            model
        })
        .collect::<Vec<_>>();
    if models.len() < entries.len() {
        truncated = true;
    }
    let mut value = Json::object();
    value.set("kind", Json::str("list_models"));
    if let Some(current) = current {
        value.set("current", Json::str(current));
    }
    value.set("models", Json::Array(models));
    if truncated {
        value.set("truncated", Json::Bool(true));
    }
    value
}

/// 保序 JSON → `serde_json::Value`（工具结果载荷用）。
pub fn to_value(value: &Json) -> serde_json::Value {
    serde_json::from_str(&value.compact()).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
#[path = "model_catalog_tests.rs"]
mod tests;
