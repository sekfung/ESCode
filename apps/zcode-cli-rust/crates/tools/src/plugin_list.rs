//! `plugins/list` 读面（docs/specs/rust-plugins.md 第 1 期）：
//! 对齐 TS `bootstrap/src/zcode-protocol/plugins.ts::listPlugins` + `toPluginInfo` +
//! `createMissingConfiguredPluginInfos`。第 1 期只输出「发现层 + 清单 + 计数 + MCP 名」，
//! 选项面（userConfig/configuredOptions/optionSources）、hookDetails、enabledSource/rootSource
//! 与 components 分组留给后续期（schema 里都是可选字段，缺失好过伪造）。

use super::{extension_config as config, extension_plugins as plugins, mcp_config};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

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

/// TS `enumeratePluginComponents`：分组顺序 agent → command → skill → hook → mcp，空组不出现。
/// 组件名/描述来自 loader 的枚举（与启用态无关）。
async fn components(plugin: &plugins::Plugin) -> Result<Value> {
    let mut groups: Vec<Value> = vec![];
    for (kind, field, default_dir) in [
        ("agent", "agents", "agents"),
        ("command", "commands", "commands"),
    ] {
        let items = markdown_components(plugin, field, default_dir).await?;
        if !items.is_empty() {
            groups.push(json!({"kind": kind, "items": items}));
        }
    }
    let skills = skill_components(plugin).await?;
    if !skills.is_empty() {
        groups.push(json!({"kind": "skill", "items": skills}));
    }
    // hook 组留到第 4 期：TS 用 loader 的 hook 源发现（manifest hooks + hook 文件，且要过
    // `canRunPluginHooks`）而不是直接读 manifest 键，本机 fixture 里 manifest 声明并不出现在
    // Node 的列表里。宁可少一组，也不输出 Node 不会显示的名字。
    let mcp: Vec<Value> = mcp_config::plugin_definitions(plugin)
        .await?
        .keys()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| json!({"name": name}))
        .collect();
    if !mcp.is_empty() {
        groups.push(json!({"kind": "mcp", "items": mcp}));
    }
    Ok(Value::Array(groups))
}

/// TS `collectMarkdownComponents`（agent / command）：先取 manifest 对象形式的声明（带描述），
/// 再扫默认目录与 manifest 声明的路径下的 `.md`，名字取 frontmatter `name` 或文件名，按名去重。
async fn markdown_components(
    plugin: &plugins::Plugin,
    field: &str,
    default_dir: &str,
) -> Result<Vec<Value>> {
    let mut items: Vec<Value> = vec![];
    let mut seen: Vec<String> = vec![];
    if let Some(declared) = plugin.manifest[field].as_object() {
        for (raw_name, meta) in declared {
            let name = raw_name.trim().to_owned();
            if name.is_empty() || seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let description = meta["description"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            items.push(match description {
                Some(description) => json!({"name": name, "description": description}),
                None => json!({"name": name}),
            });
        }
    }
    for dir in component_roots(plugin, default_dir) {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        let mut files: Vec<PathBuf> = vec![];
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().await?.is_file() && name.ends_with(".md") {
                files.push(entry.path());
            }
        }
        files.sort();
        for file in files {
            let fallback = file
                .file_name()
                .map(|name| name.to_string_lossy().trim_end_matches(".md").to_owned())
                .unwrap_or_default();
            let frontmatter = read_frontmatter(&file).await;
            let name = frontmatter.0.unwrap_or(fallback);
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            items.push(match frontmatter.1 {
                Some(description) => json!({"name": name, "description": description}),
                None => json!({"name": name}),
            });
        }
    }
    Ok(items)
}

/// TS `collectSkillComponents`：默认 `skills/` 与 manifest 声明路径下的 SKILL.md，
/// 名字取 frontmatter `name` 或技能目录名；按文件路径与最终名字双重去重。
async fn skill_components(plugin: &plugins::Plugin) -> Result<Vec<Value>> {
    let mut items: Vec<Value> = vec![];
    let mut seen_files: Vec<PathBuf> = vec![];
    let mut seen_names: Vec<String> = vec![];
    for dir in component_roots(plugin, "skills") {
        for file in skill_files_under(&dir).await? {
            if seen_files.contains(&file) {
                continue;
            }
            seen_files.push(file.clone());
            let fallback = file
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let frontmatter = read_frontmatter(&file).await;
            let name = frontmatter.0.unwrap_or(fallback);
            if seen_names.contains(&name) {
                continue;
            }
            seen_names.push(name.clone());
            items.push(match frontmatter.1 {
                Some(description) => json!({"name": name, "description": description}),
                None => json!({"name": name}),
            });
        }
    }
    Ok(items)
}

/// 与 `count_skill_files` 相同的两级扫描，返回命中的 SKILL.md 路径。
async fn skill_files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = vec![];
    if tokio::fs::symlink_metadata(root)
        .await
        .is_ok_and(|m| m.file_type().is_symlink())
        || !tokio::fs::metadata(root).await.is_ok_and(|m| m.is_dir())
    {
        return Ok(files);
    }
    let mut candidates = vec![root.join("SKILL.md")];
    if let Ok(mut entries) = tokio::fs::read_dir(root).await {
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if should_walk(&name) {
                candidates.push(entry.path().join("SKILL.md"));
            }
        }
    }
    candidates.sort();
    for candidate in candidates {
        if tokio::fs::symlink_metadata(&candidate)
            .await
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            continue;
        }
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|m| m.is_file())
        {
            files.push(candidate);
        }
    }
    Ok(files)
}

/// TS `readMarkdownFrontmatter` 的 name/description 抽取（块标量交给现有 frontmatter 解析器，
/// 再按 TS `parseScalar` 去掉成对引号）。
async fn read_frontmatter(file: &Path) -> (Option<String>, Option<String>) {
    let Ok(content) = tokio::fs::read_to_string(file).await else {
        return (None, None);
    };
    let (present, fields, _) = crate::domain::skills::frontmatter(&content);
    if !present {
        return (None, None);
    }
    let scalar = |key: &str| {
        fields
            .get(key)
            .map(|value| value.trim())
            .map(|value| {
                let quoted = (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''));
                if quoted && value.len() >= 2 {
                    value[1..value.len() - 1].trim().to_owned()
                } else {
                    value.to_owned()
                }
            })
            .filter(|value| !value.is_empty())
    };
    (scalar("name"), scalar("description"))
}


/// TS `toPluginInfo` 的 author/authorUrl 回退字段：manifest 里可以是字符串（名字）或
/// `{name, url}` 对象；其余形态忽略。
fn author_of(manifest: &Value) -> Option<(String, Option<String>)> {
    let author = manifest.get("author")?;
    if let Some(name) = author.as_str().map(str::trim).filter(|n| !n.is_empty()) {
        return Some((name.to_owned(), None));
    }
    let name = author["name"].as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let url = author["url"]
        .as_str()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);
    Some((name.to_owned(), url))
}

/// TS `resolveComponentRoots`：默认目录（存在才加）在前，manifest 声明的字符串/数组路径在后，
/// 按解析后的绝对路径去重；声明路径即使不存在也计入根数（与 TS 相同）。
fn component_roots(plugin: &plugins::Plugin, key: &str) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![];
    let default = plugin.root.join(key);
    if default.is_dir() && !roots.contains(&default) {
        roots.push(default);
    }
    for raw in config::strings(&plugin.manifest[key]) {
        let path = config::resolve(&plugin.root, raw.trim_start_matches("./"));
        if path.starts_with(&plugin.root) && !roots.contains(&path) {
            roots.push(path);
        }
    }
    roots
}

/// TS `materializeCommandMetadataRoot`：manifest `commands` 写成对象（内联命令元数据）时会额外
/// 生成一个命令根，计入 `commandRootCount`。这里只要能写出至少一条就按 1 计。
fn generated_command_root(plugin: &plugins::Plugin) -> usize {
    let Some(spec) = plugin.manifest["commands"].as_object() else {
        return 0;
    };
    spec.values()
        .any(|meta| {
            let source = meta["source"].is_string();
            let content = meta["content"].is_string();
            source ^ content
        })
        .into()
}

/// TS `countSkillFiles` + `scanSkillFilesUnderRootSync(followSymbolicLinks: false)`：
/// 根自身的 SKILL.md 与一级子目录的 SKILL.md（真实文件，不跟随符号链接）。
async fn count_skill_files(roots: &[PathBuf]) -> Result<usize> {
    let mut files: Vec<PathBuf> = vec![];
    for root in roots {
        if tokio::fs::symlink_metadata(root)
            .await
            .is_ok_and(|m| m.file_type().is_symlink())
            || !tokio::fs::metadata(root).await.is_ok_and(|m| m.is_dir())
        {
            continue;
        }
        let mut candidates = vec![root.join("SKILL.md")];
        if let Ok(mut entries) = tokio::fs::read_dir(root).await {
            while let Some(entry) = entries.next_entry().await? {
                if !entry.file_type().await?.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !should_walk(&name) {
                    continue;
                }
                candidates.push(entry.path().join("SKILL.md"));
            }
        }
        for candidate in candidates {
            if files.contains(&candidate) {
                continue;
            }
            if tokio::fs::symlink_metadata(&candidate)
                .await
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                continue;
            }
            if tokio::fs::metadata(&candidate)
                .await
                .is_ok_and(|m| m.is_file())
            {
                files.push(candidate);
            }
        }
    }
    Ok(files.len())
}

/// 与 TS `shouldWalkSkillDirectoryEntry` 同义：跳过隐藏目录与常见构建/依赖目录。
fn should_walk(name: &str) -> bool {
    if name.starts_with('.') && name != ".system" {
        return false;
    }
    ![
        "node_modules",
        "dist",
        "build",
        "out",
        "target",
        "vendor",
        "coverage",
        "__pycache__",
    ]
    .contains(&name)
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

/// `plugins/list` 的入口参数（TS `zcodePluginsListParamsSchema`）：workspace 必填、configScope 可选。
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
/// `setZCodePluginEnabled`。
///
/// - 选择器：先按完整 id，再按唯一 `name`（重名报歧义），在「全部已发现插件」里找（配置视图含项目层）。
/// - 写入目标：`scope: "workspace"` 固定写 `<workspace>/.zcode/config.json`，否则写用户层
///   `~/.zcode/cli/config.json`；只补丁 `plugins.enabledPlugins[id]`，文件其余内容与顺序保留。
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

/// TS `resolvePluginConfigPath`：workspace scope 固定 `<workspace>/.zcode/config.json`，否则用户层。
pub(super) fn config_path(cwd: &Path, scope: Option<&str>) -> PathBuf {
    if scope == Some("workspace") {
        cwd.join(".zcode").join("config.json")
    } else {
        config::home()
            .join(".zcode")
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
