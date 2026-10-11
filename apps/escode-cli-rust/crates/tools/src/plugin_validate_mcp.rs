//! 插件 MCP 声明校验（TS plugins/mcp.ts：loadPluginMcpServerDefinitions + resolvePluginMcpServers），
//! 供 `plugins/validate` 与 `plugins/describe` 共用。校验时 env / options 均为空，只收集诊断。

use super::plugin_install as install;
use super::plugin_validate::diag;
use crate::domain::json_order::Json;
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;

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

pub(super) fn diagnostics(root: &Path, manifest: &Json, id: &str) -> Vec<Value> {
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

pub(super) struct VariableContext<'a> {
    pub(super) manifest: &'a Json,
    pub(super) defaults: Vec<(String, Json)>,
}

/// (是否变量错误, 文案)。
pub(super) type Check = Result<(), (bool, String)>;

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
pub(super) fn check_server(key: &str, server: &Json, context: &VariableContext) -> Check {
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
            "MCP server {key}: escode_official auth requires type \"http\" or \"stdio\", got \"{kind}\""
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
                "MCP server {key}: escode_official auth cannot be combined with oauth"
            ));
        }
        // 宿主默认 env 在前（值是路径，不含模板），manifest env 覆盖同名项、新键追加在后。
        const DEFAULT_ENV: [&str; 6] = [
            "CLAUDE_PROJECT_DIR",
            "ESCODE_PLUGIN_DATA",
            "ESCODE_PLUGIN_ROOT",
            "ESCODE_PROJECT_DIR",
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
                "MCP server {key}: escode_official auth cannot be combined with oauth"
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
pub(super) fn template(value: &str, context: &VariableContext, allow_sensitive: bool) -> Check {
    let missing = |message: String| Err((true, message));
    for capture in TEMPLATE.captures_iter(value) {
        let name = &capture[1];
        match name {
            "CLAUDE_PLUGIN_ROOT" | "ESCODE_PLUGIN_ROOT" | "CLAUDE_PLUGIN_DATA"
            | "ESCODE_PLUGIN_DATA" | "CLAUDE_PROJECT_DIR" | "ESCODE_PROJECT_DIR" => continue,
            "CLAUDE_CODE_SESSION_ID" | "CLAUDE_SESSION_ID" | "ESCODE_SESSION_ID" => {
                return missing(format!(
                    "Plugin variable requires a runtime session context: {name}"
                ));
            }
            "CLAUDE_SKILL_DIR" | "ESCODE_SKILL_DIR" => {
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
        if name.starts_with("ESCODE_") || (allow_sensitive && ENV_NAME.is_match(name)) {
            return missing(format!("Missing environment variable: {name}"));
        }
    }
    Ok(())
}
