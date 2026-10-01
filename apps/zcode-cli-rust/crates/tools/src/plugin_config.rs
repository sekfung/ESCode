//! `plugins/configure` / `plugins/resetConfig`（docs/specs/rust-plugins.md 第 4 期选项面）：对齐 TS
//! `configurePlugin` → `configureZCodePlugin` → `updatePluginOptionsInFileConfig` 与
//! `resetPluginConfig` → `resetZCodePluginConfig`。

use super::config_file;
use super::{extension_config as config, extension_plugins as plugins, plugin_list};
use crate::domain::json_order::Json;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// 写 `plugins.options[id]`：只接受 string / number / boolean 值（TS `normalizePluginOptions`），
/// `clearOptionKeys` trim 后去空去重；`dryRun` 只做插件解析校验、不落盘。
pub(super) async fn configure(
    params: &Value,
    raw_params: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let plugin_id = plugin_list::non_empty(params, "pluginId")?;
    let input = params["options"]
        .as_object()
        .context("options must be an object")?;
    let scope = plugin_list::scope_of(params)?;
    let mut clear: Vec<String> = vec![];
    for key in params["clearOptionKeys"].as_array().into_iter().flatten() {
        // 协议 schema（nonEmptyString = trim + min(1)）：trim 后为空则整次调用 Invalid params。
        let key = key
            .as_str()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .context("Invalid params — clearOptionKeys must contain non-empty strings")?;
        if !key.is_empty() && !clear.iter().any(|k| k == key) {
            clear.push(key.to_owned());
        }
    }
    let config = config::load(&cwd).await?;
    let discovered = plugins::all(&cwd, &config, cancel).await?;
    let plugin = plugin_list::select(plugin_id, &discovered)?;
    if params["dryRun"] != true {
        // 新增键按请求里的书写顺序落盘（TS 展开对象保序）：优先从原始 params 文本保序解析，
        // 没有原文时退回 serde_json（字典序）。
        let ordered = raw_params
            .and_then(Json::parse)
            .and_then(|raw| match raw.get("options") {
                Some(Json::Object(entries)) => Some(entries.clone()),
                _ => None,
            });
        let entries = ordered.unwrap_or_else(|| {
            input
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), Json::parse(&value.to_string())?)))
                .collect()
        });
        let options: Vec<(String, Json)> = entries
            .into_iter()
            .filter(|(_, value)| matches!(value, Json::String(_) | Json::Number(_) | Json::Bool(_)))
            .collect();
        let path = plugin_list::config_path(&cwd, scope);
        let mut file = config_file::read_object_or_empty(&path).await?;
        config_file::patch_plugin_options(&mut file, &plugin.id, &options, &clear);
        config_file::atomic_write(&path, &file).await?;
    }
    Ok(json!({ "pluginId": plugin_id, "diagnostics": [] }))
}

/// workspace scope 只删启用覆盖（「恢复继承」不能顺手抹掉 workspace 的选项/密钥）；user scope 删启用覆盖与
/// 选项。没有可删的内容时不写文件。不解析插件：配置里的 id 即使插件已不存在也能清掉。
pub(super) async fn reset(params: &Value) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let plugin_id = plugin_list::non_empty(params, "pluginId")?;
    let scope = plugin_list::scope_of(params)?;
    let path = plugin_list::config_path(&cwd, scope);
    let mut file = config_file::read_object_or_empty(&path).await?;
    let changed = if scope == Some("workspace") {
        config_file::remove_plugin_enabled(&mut file, plugin_id)
    } else {
        config_file::remove_plugin(&mut file, plugin_id)
    };
    if changed {
        config_file::atomic_write(&path, &file).await?;
    }
    Ok(json!({ "pluginId": plugin_id, "diagnostics": [] }))
}
