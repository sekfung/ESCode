//! `plugins/list` 读面（docs/specs/rust-plugins.md 第 1 期）：
//! 对齐 TS `bootstrap/src/escode-protocol/plugins.ts::listPlugins` + `toPluginInfo` +
//! `createMissingConfiguredPluginInfos`。第 1 期只输出「发现层 + 清单 + 计数 + MCP 名」，
//! 选项面（userConfig/configuredOptions/optionSources）、hookDetails、enabledSource/rootSource
//! 与 components 分组留给后续期（schema 里都是可选字段，缺失好过伪造）。

use super::{extension_config as config, extension_plugins as plugins, mcp_config};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub(super) use super::plugin_list_components::{
    author_of, component_roots, components, count_skill_files, generated_command_root,
    markdown_components, read_frontmatter, should_walk, skill_components, skill_files_under,
};

pub(super) async fn list(
    cwd: &Path,
    layers: &config::Layers,
    cancel: &CancellationToken,
) -> Result<Value> {
    let merged = layers.merged();
    let config = &merged;
    let data_root = config::storage(config).join("data");
    let mut items: Vec<Value> = vec![];
    let mut seen: Vec<String> = vec![];
    let (discovered, diagnostics) = plugins::discover(cwd, config, cancel).await?;
    for plugin in &discovered {
        seen.push(plugin.id.clone());
        let mut item = info(plugin, cwd, config, &data_root).await?;
        with_sources(&mut item, plugin, layers, cwd);
        items.push(item);
    }
    let mut missing_rows = missing(&config["plugins"], &seen);
    for row in &mut missing_rows {
        let id = row["id"].as_str().unwrap_or_default().to_owned();
        add_config_sources(row, &id, layers);
    }
    items.extend(missing_rows);
    Ok(json!({ "plugins": items, "diagnostics": diagnostics }))
}

/// TS `toPluginInfo`（不带 configResult 的部分）：单个已发现插件的协议投影。
pub(super) async fn info(
    plugin: &plugins::Plugin,
    cwd: &Path,
    config: &Value,
    data_root: &Path,
) -> Result<Value> {
    let mut info = json!({
        "id": plugin.id,
        "name": plugin.name,
        "enabled": plugin.enabled,
        "source": plugin.source,
        "marketplace": plugin.marketplace,
        "rootPath": plugin.root.to_string_lossy(),
        // TS `toPluginInfo` 始终带 skillCount：停用插件走 emptyComponents 得 0。
        "skillCount": 0,
        "skillRootCount": 0,
        "commandRootCount": 0,
        "mcpServerNames": [],
    });
    let manifest = &plugin.manifest;
    if let Some(description) = manifest["description"].as_str() {
        info["description"] = description.into();
    }
    if let Some(version) = manifest["version"].as_str() {
        info["version"] = version.into();
    }
    if let Some((author, url)) = author_of(manifest) {
        info["author"] = author.into();
        if let Some(url) = url {
            info["authorUrl"] = url.into();
        }
    }
    if let Some(homepage) = manifest["homepage"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
    {
        info["homepage"] = homepage.into();
    }
    let declared: Vec<Value> = mcp_config::plugin_definitions(plugin)
        .await?
        .keys()
        .map(|name| Value::from(name.clone()))
        .collect();
    info["declaredMcpServerNames"] = Value::Array(declared);
    if plugin.enabled {
        // 与 TS 一致：只有启用插件才解析真实组件（停用插件计数为 0、MCP 名为空），
        // 但 `components` 清单与启用态无关（见 TS createPluginMetadata 注释）。
        let skill_roots = component_roots(plugin, "skills");
        let command_roots = component_roots(plugin, "commands");
        info["skillRootCount"] = json!(skill_roots.len());
        info["commandRootCount"] = json!(command_roots.len() + generated_command_root(plugin));
        info["skillCount"] = json!(count_skill_files(&skill_roots).await?);
        let mut names: Vec<Value> = vec![];
        for (name, server) in mcp_config::plugin_servers(plugin, cwd, data_root).await? {
            if !server.invalid {
                names.push(name.into());
            }
        }
        info["mcpServerNames"] = Value::Array(names);
    }
    // `components` 与启用态无关：TS `createPluginMetadata` 始终对插件根做权威枚举。
    info["components"] = components(plugin).await?;
    // TS createPluginMetadata：userConfig 是 manifest 的选项 schema 原样；configuredOptions 取有效配置里
    // 该插件的选项，按 schema 剔除 sensitive 键（脱敏合同：密钥不回传 UI），为空时不输出。
    let user_config = &plugin.manifest["userConfig"];
    if !user_config.is_null() && user_config != &Value::Bool(false) {
        info["userConfig"] = user_config.clone();
    }
    if let Some(options) = config["plugins"]["options"][&plugin.id].as_object() {
        let visible: serde_json::Map<String, Value> = options
            .iter()
            .filter(|(key, _)| user_config[key.as_str()]["sensitive"] != true)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if !visible.is_empty() {
            info["configuredOptions"] = Value::Object(visible);
        }
    }
    Ok(info)
}

/// TS `toPluginInfo` 带 configResult 的部分：`enabledSource` / `optionSources` / inline 插件的 `rootSource`。
fn with_sources(info: &mut Value, plugin: &plugins::Plugin, layers: &config::Layers, cwd: &Path) {
    add_config_sources(info, &plugin.id, layers);
    if plugin.source == "inline" {
        // TS resolveInlinePluginRootSource：workspace 声明优先；Windows 路径不区分大小写、斜杠方向不定。
        let key = |path: &Path| {
            let text = super::lexical_path::normalize(path)
                .to_string_lossy()
                .into_owned();
            if cfg!(windows) {
                text.replace('\\', "/").to_lowercase()
            } else {
                text
            }
        };
        let root = key(&plugin.root);
        let declared = |layer: &Value| {
            config::strings(&layer["plugins"]["dirs"])
                .into_iter()
                .any(|dir| key(&config::resolve(cwd, dir)) == root)
        };
        if declared(&layers.project) {
            info["rootSource"] = "workspace".into();
        } else if declared(&layers.user) {
            info["rootSource"] = "user".into();
        }
    }
}

/// TS `resolvePluginConfigSources`：项目层覆盖用户层；optionSources 按 option key 记来源，为空不输出。
fn add_config_sources(info: &mut Value, id: &str, layers: &config::Layers) {
    let mut enabled = None;
    let mut options = serde_json::Map::new();
    for (layer, scope) in [(&layers.user, "user"), (&layers.project, "workspace")] {
        if layer["plugins"]["enabledPlugins"].get(id).is_some() {
            enabled = Some(scope);
        }
        if let Some(keys) = layer["plugins"]["options"][id].as_object() {
            for key in keys.keys() {
                options.insert(key.clone(), scope.into());
            }
        }
    }
    if let Some(scope) = enabled {
        info["enabledSource"] = scope.into();
    }
    if !options.is_empty() {
        info["optionSources"] = Value::Object(options);
    }
}

/// TS `createMissingConfiguredPluginInfos`：配置里声明过、但本次没有发现的插件补一行
/// `source: "missing"` + `packageStatus: "missing"`，避免「开关还在、列表里却消失了」。
fn missing(config: &Value, discovered: &[String]) -> Vec<Value> {
    let plugins = &config;
    let mut ids: Vec<String> = vec![];
    for key in ["enabledPlugins", "options"] {
        if let Some(entries) = plugins[key].as_object() {
            for id in entries.keys() {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
    }
    let mut rows = vec![];
    for id in ids {
        if discovered.contains(&id) {
            continue;
        }
        let Some((name, marketplace)) = id.rsplit_once('@') else {
            continue;
        };
        if name.is_empty() || marketplace.is_empty() {
            continue;
        }
        let enabled = plugins["enabledPlugins"][&id].as_bool().unwrap_or(false);
        rows.push(json!({
            "id": id,
            "name": name,
            "enabled": enabled,
            "source": "missing",
            "marketplace": marketplace,
            "skillCount": 0,
            "skillRootCount": 0,
            "commandRootCount": 0,
            "components": [],
            "declaredMcpServerNames": [],
            "mcpServerNames": [],
            "rootPath": "",
            "packageStatus": "missing",
        }));
    }
    rows
}

/// `plugins/list` 的入口参数（TS `escodePluginsListParamsSchema`）：workspace 必填、configScope 可选。
pub(super) fn workspace_path(params: &Value) -> Result<PathBuf> {
    Ok(PathBuf::from(
        params["workspace"]["workspacePath"]
            .as_str()
            .context("Workspace path required")?,
    ))
}

/// 配置视图：`configScope: "user"` 时不加载项目层（TS `createPluginConfigView` 在 user 视图不传
/// workingDirectory，避免把项目 override 投影成用户当前值）。
pub(super) async fn layers_for(cwd: &Path, params: &Value) -> Result<config::Layers> {
    if params["configScope"].as_str() == Some("user") {
        Ok(config::Layers {
            user: config::load_user().await?,
            project: json!({}),
        })
    } else {
        config::load_layers(cwd).await
    }
}

pub(super) async fn config_for(cwd: &Path, params: &Value) -> Result<Value> {
    Ok(layers_for(cwd, params).await?.merged())
}

/// `plugins/setEnabled`（docs/specs/rust-plugins.md 第 2 期）：对齐 TS `setPluginEnabled` +
/// `setESCodePluginEnabled`。
///
/// - 选择器：先按完整 id，再按唯一 `name`（重名报歧义），在「全部已发现插件」里找（配置视图含项目层）。
/// - 写入目标：`scope: "workspace"` 固定写 `<workspace>/.escode/config.json`，否则写用户层
///   `~/.escode/cli/config.json`；只补丁 `plugins.enabledPlugins[id]`，文件其余内容与顺序保留。
/// - 返回值与 TS 一样基于**写入前**解析的插件元数据（停用 → 启用时计数仍为 0，等下一次 list 刷新），
///   只覆盖 `enabled` 并带上 `enabledSource = scope ?? "user"`。
pub(super) async fn set_enabled(params: &Value, cancel: &CancellationToken) -> Result<Value> {
    let cwd = workspace_path(params)?;
    let selector = non_empty(params, "pluginId")?;
    let enabled = params["enabled"]
        .as_bool()
        .context("enabled must be a boolean")?;
    let scope = scope_of(params)?;
    let config = config::load(&cwd).await?;
    let discovered = plugins::all(&cwd, &config, cancel).await?;
    let plugin = select(selector, &discovered)?;
    let path = config_path(&cwd, scope);
    let mut file = super::config_file::read_object_or_empty(&path).await?;
    super::config_file::patch_plugin_enabled(&mut file, &plugin.id, enabled);
    super::config_file::atomic_write(&path, &file).await?;
    let data_root = config::storage(&config).join("data");
    let mut info = info(plugin, &cwd, &config, &data_root).await?;
    info["enabled"] = enabled.into();
    info["enabledSource"] = scope.unwrap_or("user").into();
    Ok(json!({ "plugin": info, "enabled": enabled }))
}

/// TS `resolvePluginSelector`。
pub(super) fn select<'a>(
    selector: &str,
    plugins: &'a [plugins::Plugin],
) -> Result<&'a plugins::Plugin> {
    let selector = selector.trim();
    if let Some(plugin) = plugins.iter().find(|plugin| plugin.id == selector) {
        return Ok(plugin);
    }
    let mut matches = plugins.iter().filter(|plugin| plugin.name == selector);
    match (matches.next(), matches.next()) {
        (Some(plugin), None) => Ok(plugin),
        (Some(_), Some(_)) => {
            anyhow::bail!("Plugin name is ambiguous, use full plugin id: {selector}")
        }
        _ => anyhow::bail!("Plugin not found: {selector}"),
    }
}

/// 协议参数里的 `scope`（`"user"` | `"workspace"`，缺省 user）。
pub(super) fn scope_of(params: &Value) -> Result<Option<&str>> {
    match &params["scope"] {
        Value::Null => Ok(None),
        Value::String(scope) if scope == "user" || scope == "workspace" => Ok(Some(scope.as_str())),
        _ => anyhow::bail!("scope must be \"user\" or \"workspace\""),
    }
}

/// TS `resolvePluginConfigPath`：workspace scope 固定 `<workspace>/.escode/config.json`，否则用户层。
pub(super) fn config_path(cwd: &Path, scope: Option<&str>) -> PathBuf {
    if scope == Some("workspace") {
        cwd.join(".escode").join("config.json")
    } else {
        config::home()
            .join(".escode")
            .join("cli")
            .join("config.json")
    }
}

/// 协议 `nonEmptyString`（`z.string().trim().min(1)`）：返回 trim 后的值，trim 后为空即 Invalid params。
pub(super) fn non_empty<'a>(params: &'a Value, field: &str) -> Result<&'a str> {
    params[field]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("Invalid params — {field} must be a non-empty string"))
}
