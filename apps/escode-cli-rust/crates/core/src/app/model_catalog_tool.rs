//! `ListModels`（docs/specs/rust-dynamic-workflow.md 第 6 期）：列本宿主已配置的模型，供一次
//! workflow run 挑子代理模型（`subagent_model`）。目录来自 Provider Registry 的目录面（每轮取一次），
//! 投影与两个面在 `domain::model_catalog`。
//!
//! 端口缺席（没有注册表）时**绝不**静默回空列表：那会让模型把「这台机器没配模型」与「这个会话读不到
//! 目录」混成一个结论（TS `model_catalog_unavailable` 同款）。
use crate::contract::ToolOutput;
use crate::domain::model_catalog::{self, Entry};
use anyhow::Result;
use serde_json::Value;

fn entries(catalog: &[Value]) -> Vec<Entry> {
    catalog
        .iter()
        .map(|entry| Entry {
            provider_id: entry["providerId"].as_str().unwrap_or_default().to_owned(),
            model_id: entry["modelId"].as_str().unwrap_or_default().to_owned(),
            provider_label: entry["providerLabel"].as_str().map(str::to_owned),
            reasoning_levels: entry["reasoningLevels"]
                .as_array()
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            default_reasoning_level: entry["defaultReasoningLevel"]
                .as_str()
                .map(str::to_owned),
            context_window: entry["contextWindow"].as_i64(),
            disabled_reason: entry["disabledReason"].as_str().map(str::to_owned),
        })
        .collect()
}

pub(super) fn execute(
    catalog: Option<&[Value]>,
    selection: Option<&crate::contract::ModelIdentity>,
) -> Result<ToolOutput> {
    let Some(catalog) = catalog else {
        // TS `model_catalog_unavailable`（业务失败码 31）：能力缺口，不是"一个都没配"。
        return Ok(ToolOutput {
            failed: true,
            ..ToolOutput::text(
                "model_catalog_unavailable: this session cannot list models — the host did not provide a model catalog. This is a capability gap, not an empty configuration. Omit `subagent_model` on CreateWorkflow and AmendWorkflow; the workflow's subagents will run on the session model."
                    .to_owned(),
            )
        });
    };
    let entries = entries(catalog);
    // current 与注册表里的 providerId/modelId 两段相等即当前选择；options 不是身份的一部分。
    let current = selection
        .and_then(|selection| {
            entries
                .iter()
                .find(|entry| {
                    entry.provider_id == selection.provider_id
                        && entry.model_id == selection.model_id
                })
                .map(Entry::id)
        });
    let output = model_catalog::output(current.as_deref(), &entries);
    let mut result = ToolOutput::new(
        model_catalog::model_content(current.as_deref(), &entries),
        model_catalog::to_value(&output),
    );
    result.display = Some(model_catalog::to_value(&model_catalog::display(
        current.as_deref(),
        &entries,
    )));
    Ok(result)
}
