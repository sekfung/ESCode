//! `plugins/validate`（docs/specs/rust-plugin-marketplace-write.md W5b）：对齐 TS `validatePlugin` →
//! `validateZCodePlugin` → adapters `validateMarketplaceSource` / `validateMarketplacePlugin` /
//! `validatePluginRoot`。只读：远端源物化到临时目录后即删除，不写存储。

use super::plugin_install as install;
use super::plugin_marketplace as market;
use super::plugin_uninstall::storage_lock;
use super::{extension_config as config, plugin_list, plugin_market_write as market_write};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const UNSUPPORTED_MANIFEST_FIELDS: [&str; 4] =
    ["channels", "lspServers", "outputStyles", "settings"];

pub(super) async fn validate(params: &Value) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let field = |key: &str| plugin_list::non_empty(params, key).ok().map(str::to_owned);
    let (name, marketplace, source) = (field("pluginName"), field("marketplace"), field("source"));
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    market::ensure_default_marketplaces(&storage)?;
    let diagnostics = if let Some(source) = source {
        match market_write::parse_source_input(&source, &cwd) {
            Ok(source) => validate_source(&storage, &source).await,
            Err(error) => vec![diag(
                "plugin_marketplace_invalid",
                error.to_string(),
                None,
                true,
            )],
        }
    } else if let (Some(name), Some(marketplace)) = (name, marketplace) {
        let id = format!("{name}@{marketplace}");
        match market_write::ensure_manifest(&storage, &marketplace).await {
            Err(error) => vec![diag(
                "plugin_marketplace_invalid",
                error.to_string(),
                Some(&id),
                true,
            )],
            Ok(()) => {
                tokio::task::spawn_blocking(move || validate_plugin(&storage, &marketplace, &name))
                    .await
                    .map_err(|_| anyhow!("Plugin validate worker panicked"))?
            }
        }
    } else {
        vec![]
    };
    let ok = diagnostics.iter().all(|d| d["severity"] != "error");
    Ok(json!({
        "ok": ok,
        "diagnostics": diagnostics,
        "compatibility": {
            "runnable": ["skills", "commands", "hooks", "mcpServers", "userConfig"],
            "diagnosticOnly": ["agents", "lspServers", "outputStyles", "channels", "settings"],
            "unsupported": ["mcpb", "dxt", "npm", "hostPattern", "pathPattern"],
        },
    }))
}

pub(super) fn diag(
    code: &str,
    message: impl Into<String>,
    plugin_id: Option<&str>,
    error: bool,
) -> Value {
    let mut out = json!({
        "code": code,
        "message": message.into(),
        "severity": if error { "error" } else { "warning" },
    });
    if let Some(id) = plugin_id {
        out["pluginId"] = id.into();
    }
    out
}

/// TS `toValidationDiagnostic`：源错误带 code；其余按文案归类。
pub(super) fn error_diagnostic(error: &anyhow::Error, plugin_id: Option<&str>) -> Value {
    let message = error.to_string();
    let code = if let Some(source) = error.downcast_ref::<super::plugin_git::SourceError>() {
        source.code
    } else if message.contains("source is recognized but not supported in this runtime") {
        "plugin_marketplace_source_unsupported"
    } else if message.contains("Cross-marketplace dependency") {
        "plugin_dependency_cross_marketplace"
    } else if message.contains("dependency cycle") {
        "plugin_dependency_cycle"
    } else if message.contains("Dependency not found")
        || message.contains("Marketplace not found for dependency")
    {
        "plugin_dependency_missing"
    } else {
        "plugin_marketplace_invalid"
    };
    diag(code, message, plugin_id, true)
}

pub(super) fn entry_name(entry: &Json) -> String {
    entry
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// TS `validateMarketplacePlugin`（阻塞线程）。
fn validate_plugin(storage: &Path, marketplace: &str, name: &str) -> Vec<Value> {
    let id = format!("{name}@{marketplace}");
    if market::manifest(storage, marketplace).is_none() {
        return vec![diag(
            "plugin_marketplace_invalid",
            format!("Marketplace not found: {marketplace}"),
            None,
            true,
        )];
    }
    let Some(entry) = install::ordered_entry(storage, marketplace, name) else {
        return vec![diag(
            "plugin_not_found",
            format!("Plugin not found: {id}"),
            None,
            true,
        )];
    };
    let mut diagnostics = vec![];
    if let Err(error) = install::closure_in(storage, marketplace, name, None) {
        diagnostics.push(error_diagnostic(&error, Some(&id)));
    }
    match install::materialize(storage, marketplace, &entry, None) {
        Ok(root) => {
            diagnostics.extend(validate_root(&entry, marketplace, &root.path));
            root.cleanup();
        }
        Err(error) => diagnostics.push(error_diagnostic(&error, Some(&id))),
    }
    diagnostics
}

/// TS `validateMarketplaceSource`（persist:false）：加载市场，逐条目校验形状 / 依赖；远端条目延后深扫。
async fn validate_source(storage: &Path, source: &Json) -> Vec<Value> {
    let loaded = match market_write::load(source).await {
        Ok(loaded) => loaded,
        Err(error) => return vec![error_diagnostic(&error, None)],
    };
    let storage = storage.to_owned();
    let raw = loaded.raw.clone();
    let name = loaded.name.clone();
    let source_root = loaded.source_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        validate_loaded(&storage, &raw, &name, source_root.as_deref())
    })
    .await
    .unwrap_or_else(|_| {
        vec![diag(
            "plugin_marketplace_invalid",
            "Plugin validate worker panicked",
            None,
            true,
        )]
    });
    if let Some(temp) = &loaded.temp {
        let _ = std::fs::remove_dir_all(temp);
    }
    result
}

fn validate_loaded(
    storage: &Path,
    raw: &Json,
    marketplace: &str,
    source_root: Option<&Path>,
) -> Vec<Value> {
    let mut diagnostics = vec![];
    let entries: Vec<&Json> = raw
        .get("plugins")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry.is_object() && !entry_name(entry).is_empty())
        .collect();
    if entries.is_empty() {
        diagnostics.push(diag(
            "plugin_marketplace_invalid",
            format!("Marketplace has no plugins: {marketplace}"),
            None,
            false,
        ));
    }
    // 相对源按市场源目录解析（TS sourceRoot ?? 存储里的市场目录）。
    let dir: PathBuf = source_root.map(Path::to_owned).unwrap_or_else(|| {
        storage
            .join("marketplaces")
            .join(market::sanitize(marketplace))
    });
    for entry in entries {
        let name = entry_name(entry);
        let id = format!("{name}@{marketplace}");
        diagnostics.extend(entry_shape(entry, &id));
        if let Err(error) = install::closure_in(storage, marketplace, &name, Some(raw)) {
            diagnostics.push(error_diagnostic(&error, Some(&id)));
        }
        if let Some(deferred) = deferral(entry, &id) {
            diagnostics.push(deferred);
            diagnostics.extend(compatibility(&entry_manifest(entry), &id));
            continue;
        }
        match install::materialize(storage, marketplace, entry, Some((&dir, raw))) {
            Ok(root) => {
                diagnostics.extend(validate_root(entry, marketplace, &root.path));
                root.cleanup();
            }
            Err(error) => {
                diagnostics.push(error_diagnostic(&error, Some(&id)));
                // 源暂时不可解析时仍基于条目原文给出 diagnostic-only 能力诊断。
                diagnostics.extend(compatibility(&entry_manifest(entry), &id));
            }
        }
    }
    diagnostics
}

/// TS `validateMarketplaceEntryShape`（includeEntryCompatibility:false）。
fn entry_shape(entry: &Json, id: &str) -> Vec<Value> {
    let mut out = vec![];
    let Some(source) = entry.get("source") else {
        return vec![diag(
            "plugin_marketplace_invalid",
            format!("Plugin has no install source: {id}"),
            Some(id),
            true,
        )];
    };
    if !source.is_object() {
        return out;
    }
    let text = |key: &str| source.get(key).and_then(Json::as_str).unwrap_or_default();
    let kind = text("source");
    if kind == "npm" || kind == "pip" {
        out.push(diag(
            "plugin_marketplace_source_unsupported",
            format!("Plugin source is recognized but not supported in V1 install: {kind}"),
            Some(id),
            false,
        ));
    }
    if kind == "url" {
        let source_type = text("type");
        if !source_type.is_empty() && source_type != "git" && source_type != "zip" {
            out.push(diag(
                "plugin_marketplace_source_unsupported",
                format!("Plugin URL source type is not supported: {source_type}"),
                Some(id),
                true,
            ));
        }
        if let Err(message) = url_shape(source, source_type == "zip") {
            out.push(diag("plugin_marketplace_invalid", message, Some(id), true));
        }
    }
    out
}

/// TS 形状校验里的 `readRequired*` / `readOptional*`（zip 源：url → sha256 → headers → path → stripRoot）。
fn url_shape(source: &Json, zip: bool) -> Result<(), String> {
    let has_text = |key: &str| {
        source
            .get(key)
            .and_then(Json::as_str)
            .is_some_and(|v| !v.trim().is_empty())
    };
    if !has_text("url") {
        return Err("Plugin URL source requires a non-empty url".into());
    }
    if !zip {
        return Ok(());
    }
    let Some(sha) = source.get("sha256").and_then(Json::as_str) else {
        return Err("Plugin zip source sha256 is required".into());
    };
    let sha = sha.to_lowercase();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Plugin zip source sha256 must be a 64 character hex string".into());
    }
    match source.get("headers") {
        None => {}
        Some(Json::Object(entries)) => {
            if let Some((key, _)) = entries.iter().find(|(_, v)| v.as_str().is_none()) {
                return Err(format!("Plugin zip source header must be a string: {key}"));
            }
        }
        Some(_) => return Err("Plugin zip source headers must be an object".into()),
    }
    if source.get("path").is_some_and(|p| p.as_str().is_none()) {
        return Err("Plugin zip source path must be a string".into());
    }
    if source
        .get("stripRoot")
        .is_some_and(|v| !matches!(v, Json::Bool(_)))
    {
        return Err("Plugin zip source stripRoot must be a boolean".into());
    }
    Ok(())
}

/// TS `getMarketplaceSourceValidationDeferral`：市场级校验不逐个 clone 远端源。
fn deferral(entry: &Json, id: &str) -> Option<Value> {
    let source = entry.get("source").filter(|s| s.is_object())?;
    let text = |key: &str| source.get(key).and_then(Json::as_str);
    let kind = text("source").unwrap_or_default();
    if kind == "url" {
        let source_type = text("type").unwrap_or_default();
        if !source_type.is_empty() && source_type != "git" && source_type != "zip" {
            return None;
        }
    }
    if !["github", "git", "url", "git-subdir"].contains(&kind) {
        return None;
    }
    let label = text("repo").or_else(|| text("url")).unwrap_or(kind);
    Some(diag(
        "plugin_validation_deferred",
        format!(
            "Remote plugin source validation is deferred until install or single-plugin validate: {label}"
        ),
        Some(id),
        false,
    ))
}

/// TS `createManifestFromMarketplaceEntry`：条目去掉来源 / 商店展示字段，补 name / version。
fn entry_manifest(entry: &Json) -> Json {
    let mut manifest = entry.clone();
    for key in [
        "source",
        "category",
        "tags",
        "strict",
        "displayName",
        "displayName_i18n",
        "description_i18n",
        "icon",
        "privacyPolicy",
        "termsOfService",
        "heroImage",
        "examplePrompts",
        "examplePrompts_i18n",
        "requiresPaidPlan",
    ] {
        manifest.remove(key);
    }
    manifest.set("name", Json::str(entry_name(entry)));
    let version = entry
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or("0.0.0")
        .to_owned();
    manifest.set("version", Json::str(version));
    manifest
}

/// TS `pushManifestCompatibilityDiagnostics`。
fn compatibility(manifest: &Json, id: &str) -> Vec<Value> {
    let mut out = vec![];
    for key in UNSUPPORTED_MANIFEST_FIELDS {
        if manifest.get(key).is_some() {
            out.push(diag(
                "plugin_unsupported_component",
                format!("Plugin component is diagnostic-only in this ZCode runtime: {key}"),
                Some(id),
                false,
            ));
        }
    }
    if let Some(Json::Object(options)) = manifest.get("userConfig") {
        for (key, option) in options {
            if option.get("required") == Some(&Json::Bool(true)) && option.get("default").is_none()
            {
                out.push(diag(
                    "plugin_variable_missing",
                    format!(
                        "Required plugin userConfig has no default and must be configured: {key}"
                    ),
                    Some(id),
                    false,
                ));
            }
        }
    }
    fn bundle(value: &Json) -> bool {
        match value {
            Json::String(text) => text.ends_with(".mcpb") || text.ends_with(".dxt"),
            Json::Array(items) => items.iter().any(bundle),
            _ => false,
        }
    }
    if manifest.get("mcpServers").is_some_and(bundle) {
        out.push(diag(
            "plugin_marketplace_source_unsupported",
            "MCPB/DXT plugin bundles are recognized but not supported in this runtime",
            Some(id),
            false,
        ));
    }
    out
}

/// TS `readPluginManifestFromRoot`：有 plugin.json 则解析（name trim + 校验，version 缺省 0.0.0）；
/// 没有且条目 `strict: false` 时用条目合成；否则 None。
pub(super) fn read_root_manifest(root: &Path, entry: &Json) -> Result<Option<Json>> {
    if let Some(path) = install::manifest_path(root) {
        let text = std::fs::read_to_string(&path)?;
        let mut parsed = Json::parse(&text)
            .ok_or_else(|| anyhow!("Unexpected token in JSON at {}", path.display()))?;
        if !parsed.is_object() {
            return Err(anyhow!("Plugin manifest must be a JSON object"));
        }
        let name = entry_name(&parsed);
        if !install::valid_plugin_name(&name) {
            return Err(anyhow!("Invalid plugin name: {name}"));
        }
        parsed.set("name", Json::str(name));
        if parsed.get("version").and_then(Json::as_str).is_none() {
            parsed.set("version", Json::str("0.0.0"));
        }
        return Ok(Some(parsed));
    }
    if entry.get("strict") == Some(&Json::Bool(false)) {
        return Ok(Some(entry_manifest(entry)));
    }
    Ok(None)
}

/// TS `validatePluginRoot`。
fn validate_root(entry: &Json, marketplace: &str, root: &Path) -> Vec<Value> {
    let name = entry_name(entry);
    let id = format!("{name}@{marketplace}");
    let manifest = match read_root_manifest(root, entry) {
        Err(error) => {
            return vec![diag(
                "plugin_manifest_invalid",
                error.to_string(),
                Some(&id),
                true,
            )];
        }
        Ok(None) => {
            return vec![diag(
                "plugin_manifest_not_found",
                format!("Plugin manifest not found: {id}"),
                Some(&id),
                true,
            )];
        }
        Ok(Some(manifest)) => manifest,
    };
    let mut out = vec![];
    let manifest_name = entry_name(&manifest);
    if manifest_name != name {
        out.push(diag(
            "plugin_manifest_invalid",
            format!(
                "Plugin manifest name '{manifest_name}' does not match marketplace entry '{name}'"
            ),
            Some(&id),
            true,
        ));
    }
    out.extend(compatibility(&manifest, &id));
    out.extend(mcp_diagnostics(root, &manifest, &id));
    out
}

// ---- 插件 MCP 声明校验（TS plugins/mcp.ts：loadPluginMcpServerDefinitions + resolvePluginMcpServers，
// 校验时 env / options 均为空，只收集诊断） ----

/// TS `loadPluginMcpServerDefinitions`：`.mcp.json` 在前，manifest `mcpServers` 覆盖同名项；读取诊断写入 `out`。
pub(super) fn mcp_definitions(
    root: &Path,
    manifest: &Json,
    id: &str,
    out: &mut Vec<Value>,
) -> Vec<(String, Json)> {
    let mut servers = mcp_file(&root.join(".mcp.json"), id, out);
    if let Some(spec) = manifest.get("mcpServers") {
        for (key, value) in mcp_spec(root, spec, id, out) {
            upsert(&mut servers, key, value);
        }
    }
    servers
}

fn mcp_diagnostics(root: &Path, manifest: &Json, id: &str) -> Vec<Value> {
    let mut out = vec![];
    let servers = mcp_definitions(root, manifest, id, &mut out);
    let defaults: Vec<(String, Json)> = match manifest.get("userConfig") {
        Some(Json::Object(options)) => options
            .iter()
            .filter_map(|(key, option)| {
                option
                    .get("default")
                    .filter(|v| matches!(v, Json::String(_) | Json::Number(_) | Json::Bool(_)))
                    .map(|v| (key.clone(), v.clone()))
            })
            .collect(),
        _ => vec![],
    };
    let context = VariableContext { manifest, defaults };
    for (key, server) in &servers {
        if let Err((variable, message)) = check_server(key, server, &context) {
            out.push(diag(
                if variable {
                    "plugin_variable_missing"
                } else {
                    "plugin_mcp_server_disabled"
                },
                message,
                Some(id),
                true,
            ));
        }
    }
    out
}

fn upsert(servers: &mut Vec<(String, Json)>, key: String, value: Json) {
    match servers.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => servers.push((key, value)),
    }
}

fn mcp_file(path: &Path, id: &str, out: &mut Vec<Value>) -> Vec<(String, Json)> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(error) => {
            out.push(diag(
                "plugin_mcp_read_failed",
                error.to_string(),
                Some(id),
                true,
            ));
            return vec![];
        }
    };
    match Json::parse(&text) {
        Some(value) => mcp_shape(&value, id, out),
        None => {
            out.push(diag(
                "plugin_mcp_read_failed",
                format!("Unexpected token in JSON at {}", path.display()),
                Some(id),
                true,
            ));
            vec![]
        }
    }
}

fn mcp_spec(root: &Path, spec: &Json, id: &str, out: &mut Vec<Value>) -> Vec<(String, Json)> {
    match spec {
        Json::String(path) => match install::resolve_inside(root, path) {
            Some(path) => mcp_file(&path, id, out),
            None => {
                out.push(diag(
                    "plugin_component_path_invalid",
                    format!("Plugin mcpServers path escapes plugin root: {path}"),
                    Some(id),
                    true,
                ));
                vec![]
            }
        },
        Json::Array(items) => {
            let mut merged = vec![];
            for item in items {
                for (key, value) in mcp_spec(root, item, id, out) {
                    upsert(&mut merged, key, value);
                }
            }
            merged
        }
        other => mcp_shape(other, id, out),
    }
}

fn mcp_shape(value: &Json, id: &str, out: &mut Vec<Value>) -> Vec<(String, Json)> {
    if !value.is_object() {
        out.push(diag(
            "plugin_mcp_invalid",
            "Plugin MCP config must be an object",
            Some(id),
            true,
        ));
        return vec![];
    }
    let servers = value
        .get("mcpServers")
        .filter(|s| s.is_object())
        .unwrap_or(value);
    match servers {
        Json::Object(entries) => entries
            .iter()
            .filter(|(_, config)| config.is_object())
            .cloned()
            .collect(),
        _ => vec![],
    }
}

struct VariableContext<'a> {
    manifest: &'a Json,
    defaults: Vec<(String, Json)>,
}

/// (是否变量错误, 文案)。
type Check = Result<(), (bool, String)>;

fn fail(message: impl Into<String>) -> Check {
    Err((false, message.into()))
}

/// JS `String(value)` 的常见情形。
fn js_string(value: Option<&Json>) -> String {
    match value {
        None => "undefined".into(),
        Some(Json::Null) => "null".into(),
        Some(Json::String(text)) => text.clone(),
        Some(Json::Bool(b)) => b.to_string(),
        Some(Json::Object(_)) => "[object Object]".into(),
        Some(other) => other.compact(),
    }
}

/// TS `resolveMcpServerConfig` 的报错顺序（只校验，不产出配置）。
fn check_server(key: &str, server: &Json, context: &VariableContext) -> Check {
    let text = |field: &str| server.get(field).and_then(Json::as_str);
    let kind = text("type").map(str::to_owned).unwrap_or_else(|| {
        if text("command").is_some() {
            "stdio".into()
        } else {
            "http".into()
        }
    });
    if !["stdio", "http", "sse"].contains(&kind.as_str()) {
        return fail(format!("Unsupported MCP transport: {kind}"));
    }
    let auth = server
        .get("auth")
        .map(|a| serde_json::from_str::<Value>(&a.compact()).unwrap_or(Value::Null));
    let official = crate::domain::mcp_official_auth::parse_auth(auth.as_ref(), key)
        .map_err(|message| (false, message))?;
    if official && kind != "http" && kind != "stdio" {
        return fail(format!(
            "MCP server {key}: zcode_official auth requires type \"http\" or \"stdio\", got \"{kind}\""
        ));
    }
    let required = |field: &str, message: &str| match text(field) {
        Some(value) if !value.is_empty() => Ok(value.to_owned()),
        _ => Err((false, message.to_owned())),
    };
    if kind == "stdio" {
        let command = required("command", "stdio MCP server requires command")?;
        if official && server.get("oauth").is_some() {
            return fail(format!(
                "MCP server {key}: zcode_official auth cannot be combined with oauth"
            ));
        }
        // 宿主默认 env 在前（值是路径，不含模板），manifest env 覆盖同名项、新键追加在后。
        const DEFAULT_ENV: [&str; 6] = [
            "CLAUDE_PROJECT_DIR",
            "ZCODE_PLUGIN_DATA",
            "ZCODE_PLUGIN_ROOT",
            "ZCODE_PROJECT_DIR",
            "CLAUDE_PLUGIN_DATA",
            "CLAUDE_PLUGIN_ROOT",
        ];
        if let Some(Json::Object(env)) = server.get("env") {
            let value = |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v);
            for name in DEFAULT_ENV {
                if let Some(Json::String(v)) = value(name) {
                    template(v, context, true)?;
                }
            }
            for (name, v) in env {
                if !DEFAULT_ENV.contains(&name.as_str())
                    && let Json::String(v) = v
                {
                    template(v, context, true)?;
                }
            }
        }
        template(&command, context, false)?;
        if let Some(Json::Array(args)) = server.get("args") {
            for arg in args.iter().filter_map(Json::as_str) {
                template(arg, context, false)?;
            }
        }
        if let Some(cwd) = text("cwd") {
            template(cwd, context, false)?;
        }
        return Ok(());
    }
    let url = required("url", &format!("{kind} MCP server requires url"))?;
    let mut headers = vec![];
    if let Some(Json::Object(entries)) = server.get("headers") {
        for (name, value) in entries {
            if let Json::String(value) = value {
                template(value, context, true)?;
                headers.push(name.as_str());
            }
        }
    }
    let oauth = check_oauth(server.get("oauth"), context)?;
    if official {
        if oauth {
            return fail(format!(
                "MCP server {key}: zcode_official auth cannot be combined with oauth"
            ));
        }
        let reserved = crate::domain::mcp_official_auth::reserved_headers(headers);
        if !reserved.is_empty() {
            return fail(format!(
                "MCP server {key}: static headers must not contain reserved header(s): {}",
                reserved.join(", ")
            ));
        }
    }
    template(&url, context, false)
}

/// TS `resolveMcpOAuthConfig`：返回是否声明了 oauth。
fn check_oauth(value: Option<&Json>, context: &VariableContext) -> Result<bool, (bool, String)> {
    let Some(oauth) = value.filter(|v| v.is_object()) else {
        return Ok(false);
    };
    let text = |field: &str| oauth.get(field).and_then(Json::as_str);
    let fields: &[(&str, bool)] = match text("type") {
        Some("client_credentials") => {
            for (field, label) in [("clientId", "clientId"), ("clientSecret", "clientSecret")] {
                if !text(field).is_some_and(|v| !v.is_empty()) {
                    return Err((
                        false,
                        format!("MCP OAuth client_credentials requires {label}"),
                    ));
                }
            }
            &[
                ("clientId", false),
                ("clientSecret", true),
                ("clientName", false),
                ("scope", false),
            ]
        }
        Some("authorization_code") => &[
            ("clientId", false),
            ("clientSecret", true),
            ("clientName", false),
            ("redirectPath", false),
            ("scope", false),
        ],
        _ => {
            return Err((
                false,
                format!(
                    "Unsupported MCP OAuth type: {}",
                    js_string(oauth.get("type"))
                ),
            ));
        }
    };
    for (field, sensitive) in fields {
        if let Some(value) = text(field) {
            template(value, context, *sensitive)?;
        }
    }
    Ok(true)
}

static TEMPLATE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\$\{([^}]+)\}").expect("template pattern"));
static ENV_NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("env pattern"));

/// TS `resolveTemplate`：校验时 env / options 为空，只判断是否会抛 PluginVariableError。
fn template(value: &str, context: &VariableContext, allow_sensitive: bool) -> Check {
    let missing = |message: String| Err((true, message));
    for capture in TEMPLATE.captures_iter(value) {
        let name = &capture[1];
        match name {
            "CLAUDE_PLUGIN_ROOT" | "ZCODE_PLUGIN_ROOT" | "CLAUDE_PLUGIN_DATA"
            | "ZCODE_PLUGIN_DATA" | "CLAUDE_PROJECT_DIR" | "ZCODE_PROJECT_DIR" => continue,
            "CLAUDE_CODE_SESSION_ID" | "CLAUDE_SESSION_ID" | "ZCODE_SESSION_ID" => {
                return missing(format!(
                    "Plugin variable requires a runtime session context: {name}"
                ));
            }
            "CLAUDE_SKILL_DIR" | "ZCODE_SKILL_DIR" => {
                return missing(format!("Plugin variable requires a skill context: {name}"));
            }
            _ => {}
        }
        if let Some(key) = name.strip_prefix("user_config.") {
            let sensitive = context
                .manifest
                .get("userConfig")
                .and_then(|c| c.get(key))
                .and_then(|o| o.get("sensitive"))
                == Some(&Json::Bool(true));
            if sensitive && !allow_sensitive {
                return missing(format!(
                    "Sensitive plugin user_config value cannot be used in this field: {key}"
                ));
            }
            if !context.defaults.iter().any(|(k, _)| k == key) {
                return missing(format!("Missing plugin user_config value: {key}"));
            }
            continue;
        }
        if name.starts_with("ZCODE_") || (allow_sensitive && ENV_NAME.is_match(name)) {
            return missing(format!("Missing environment variable: {name}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(manifest: &Json) -> VariableContext<'_> {
        VariableContext {
            manifest,
            defaults: vec![("region".into(), Json::str("us"))],
        }
    }

    #[test]
    fn template_rules() {
        let manifest = Json::parse(r#"{"userConfig":{"token":{"sensitive":true}}}"#).unwrap();
        let ctx = context(&manifest);
        assert!(template("${CLAUDE_PLUGIN_ROOT}/x ${user_config.region}", &ctx, false).is_ok());
        assert!(template("${HOME}", &ctx, false).is_ok());
        assert_eq!(
            template("${HOME}", &ctx, true),
            Err((true, "Missing environment variable: HOME".into()))
        );
        assert_eq!(
            template("${user_config.token}", &ctx, false),
            Err((
                true,
                "Sensitive plugin user_config value cannot be used in this field: token".into()
            ))
        );
        assert_eq!(
            template("${user_config.token}", &ctx, true),
            Err((true, "Missing plugin user_config value: token".into()))
        );
        assert_eq!(
            template("${ZCODE_SESSION_ID}", &ctx, false),
            Err((
                true,
                "Plugin variable requires a runtime session context: ZCODE_SESSION_ID".into()
            ))
        );
    }

    #[test]
    fn server_rules() {
        let manifest = Json::object();
        let ctx = context(&manifest);
        let check = |text: &str| check_server("s", &Json::parse(text).unwrap(), &ctx);
        assert!(check(r#"{"command":"node","args":["${CLAUDE_PLUGIN_ROOT}/a.js"]}"#).is_ok());
        assert_eq!(
            check(r#"{"type":"ws","url":"x"}"#),
            Err((false, "Unsupported MCP transport: ws".into()))
        );
        assert_eq!(
            check(r#"{"type":"sse"}"#),
            Err((false, "sse MCP server requires url".into()))
        );
        assert_eq!(
            check(r#"{"url":"https://x","oauth":{"type":"magic"}}"#),
            Err((false, "Unsupported MCP OAuth type: magic".into()))
        );
    }
}
