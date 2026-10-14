//! 插件 MCP 配置的变量展开（TS `adapters/src/plugins/mcp.ts` 的 `resolveTemplate` 与
//! `resolveMcpServerConfig` 的取字段顺序）。
//!
//! 修复：Rust 此前只替换插件根（`replace_root`），其余 `${...}` 原样留在配置里。v3.15.1 的官方
//! image-search 插件以 `${ESCODE_BASE_URL}` 拼 MCP URL，Rust 会连向字面量地址，`plugins/list` 也缺
//! TS 的 `plugin_variable_missing` 诊断。这里按 TS 规则展开，失败时该 MCP 失效并给出同文案诊断。

use serde_json::Value;
use std::sync::LazyLock;

static TEMPLATE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\$\{([^}]+)\}").expect("template pattern"));
static ENV_NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("env pattern"));

/// TS `VariableContext`：环境变量取进程环境（TS 传入 `process.env`）。
pub(super) struct Context<'a> {
    pub root: &'a str,
    pub data: &'a str,
    pub project: &'a str,
    pub manifest: &'a Value,
    /// 有效配置里该插件的选项（`plugins.options[<id>]`）。
    pub options: &'a Value,
}

impl Context<'_> {
    /// TS `context.options[key] ?? userConfigDefaults[key]`（默认值只认 string / number / boolean）。
    fn option(&self, key: &str) -> Option<String> {
        let primitive = |value: &Value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(js_number(number)),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        };
        self.options
            .get(key)
            .and_then(primitive)
            .or_else(|| primitive(&self.manifest["userConfig"][key]["default"]))
    }
}

/// JS `String(number)` 的常见形态：整数不带小数点。
fn js_number(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(value) if value.fract() == 0.0 && value.abs() < 1e21 => format!("{value:.0}"),
        _ => number.to_string(),
    }
}

/// TS `resolveTemplate`：返回展开结果或 PluginVariableError 的文案。
pub(super) fn resolve(
    value: &str,
    context: &Context,
    allow_sensitive: bool,
) -> Result<String, String> {
    let mut error = None;
    let resolved = TEMPLATE.replace_all(value, |capture: &regex::Captures| {
        if error.is_some() {
            return String::new();
        }
        let name = &capture[1];
        let outcome = match name {
            "CLAUDE_PLUGIN_ROOT" | "ESCODE_PLUGIN_ROOT" => Ok(context.root.to_owned()),
            "CLAUDE_PLUGIN_DATA" | "ESCODE_PLUGIN_DATA" => Ok(context.data.to_owned()),
            "CLAUDE_PROJECT_DIR" | "ESCODE_PROJECT_DIR" => Ok(context.project.to_owned()),
            "CLAUDE_CODE_SESSION_ID" | "CLAUDE_SESSION_ID" | "ESCODE_SESSION_ID" => Err(format!(
                "Plugin variable requires a runtime session context: {name}"
            )),
            "CLAUDE_SKILL_DIR" | "ESCODE_SKILL_DIR" => {
                Err(format!("Plugin variable requires a skill context: {name}"))
            }
            _ => {
                if let Some(key) = name.strip_prefix("user_config.") {
                    let sensitive = context.manifest["userConfig"][key]["sensitive"] == true;
                    if sensitive && !allow_sensitive {
                        Err(format!(
                            "Sensitive plugin user_config value cannot be used in this field: {key}"
                        ))
                    } else {
                        context
                            .option(key)
                            .ok_or_else(|| format!("Missing plugin user_config value: {key}"))
                    }
                } else if name.starts_with("ESCODE_")
                    || (allow_sensitive && ENV_NAME.is_match(name))
                {
                    // 普通 env 只在敏感 sink（env / headers / clientSecret）展开，避免 secret 进 args 或 URL。
                    escode_cli_core_api::env::var(name)
                        .map_err(|_| format!("Missing environment variable: {name}"))
                } else {
                    Ok(capture[0].to_owned())
                }
            }
        };
        outcome.unwrap_or_else(|message| {
            error = Some(message);
            String::new()
        })
    });
    match error {
        Some(message) => Err(message),
        None => Ok(resolved.into_owned()),
    }
}

/// 按 TS `resolveMcpServerConfig` 的取字段顺序原地展开一个 server：stdio 先 env（敏感）再
/// command / args / cwd；http / sse 先 headers（敏感）、oauth，最后 url。
pub(super) fn resolve_server(raw: &mut Value, context: &Context) -> Result<(), String> {
    let stdio = match raw["type"].as_str() {
        Some(kind) => kind == "stdio",
        None => raw["command"].is_string(),
    };
    let field = |raw: &mut Value, key: &str, sensitive: bool| -> Result<(), String> {
        if let Some(text) = raw.get(key).and_then(Value::as_str) {
            let value = resolve(text, context, sensitive)?;
            raw[key] = value.into();
        }
        Ok(())
    };
    let record = |value: Option<&mut Value>, sensitive: bool| -> Result<(), String> {
        if let Some(Value::Object(entries)) = value {
            for (_, entry) in entries.iter_mut() {
                if let Some(text) = entry.as_str() {
                    *entry = resolve(text, context, sensitive)?.into();
                }
            }
        }
        Ok(())
    };
    if stdio {
        record(raw.get_mut("env"), true)?;
        field(raw, "command", false)?;
        if let Some(Value::Array(args)) = raw.get_mut("args") {
            for arg in args.iter_mut() {
                if let Some(text) = arg.as_str() {
                    *arg = resolve(text, context, false)?.into();
                }
            }
        }
        return field(raw, "cwd", false);
    }
    record(raw.get_mut("headers"), true)?;
    if let Some(oauth) = raw.get_mut("oauth").filter(|v| v.is_object()) {
        for (key, sensitive) in [
            ("clientId", false),
            ("clientSecret", true),
            ("clientName", false),
            ("redirectPath", false),
            ("scope", false),
        ] {
            field(oauth, key, sensitive)?;
        }
    }
    field(raw, "url", false)
}

#[cfg(test)]
mod tests {
    use super::{Context, resolve, resolve_server};
    use serde_json::json;

    fn context<'a>(manifest: &'a serde_json::Value, options: &'a serde_json::Value) -> Context<'a> {
        Context {
            root: "/r",
            data: "/d",
            project: "/p",
            manifest,
            options,
        }
    }

    #[test]
    fn expands_like_ts_resolve_template() {
        let manifest =
            json!({"userConfig": {"port": {"default": 8080}, "token": {"sensitive": true}}});
        let options = json!({"token": "s3"});
        let ctx = context(&manifest, &options);
        assert_eq!(
            resolve("${CLAUDE_PLUGIN_ROOT}/x:${user_config.port}", &ctx, false).unwrap(),
            "/r/x:8080"
        );
        assert_eq!(resolve("${OTHER}", &ctx, false).unwrap(), "${OTHER}");
        assert_eq!(
            resolve("${user_config.token}", &ctx, false).unwrap_err(),
            "Sensitive plugin user_config value cannot be used in this field: token"
        );
        assert_eq!(resolve("${user_config.token}", &ctx, true).unwrap(), "s3");
        assert_eq!(
            resolve("${ESCODE_TEST_SURELY_UNSET_VAR}/a", &ctx, false).unwrap_err(),
            "Missing environment variable: ESCODE_TEST_SURELY_UNSET_VAR"
        );
        assert_eq!(
            resolve("${ESCODE_SESSION_ID}", &ctx, true).unwrap_err(),
            "Plugin variable requires a runtime session context: ESCODE_SESSION_ID"
        );
    }

    #[test]
    fn http_url_reports_missing_base_url() {
        let (manifest, options) = (json!({}), json!({}));
        let mut raw = json!({"type": "http", "url": "${ESCODE_TEST_SURELY_UNSET_VAR}/api"});
        assert_eq!(
            resolve_server(&mut raw, &context(&manifest, &options)).unwrap_err(),
            "Missing environment variable: ESCODE_TEST_SURELY_UNSET_VAR"
        );
        let mut raw =
            json!({"command": "${CLAUDE_PLUGIN_ROOT}/bin", "args": ["${ESCODE_PROJECT_DIR}"]});
        resolve_server(&mut raw, &context(&manifest, &options)).unwrap();
        assert_eq!(raw, json!({"command": "/r/bin", "args": ["/p"]}));
    }
}
