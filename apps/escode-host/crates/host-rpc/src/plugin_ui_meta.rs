//! MCP App 资源选择与 `_meta` 归一化（TS `pickHtmlResource` / `normalizeMcpAppResourceMeta`）。
//!
//! 资源项自己的 `_meta` 是权威；csp / prefersBorder 兼容 `openai/widget*` 别名，
//! 尺寸提示只认 `openai/widgetHeightHint` / `openai/widgetMinFrameHeight`。

use serde_json::{Map, Value, json};

/// TS `MCP_APPS_HTML_MAX_BYTES`。
const HTML_MAX_BYTES: usize = 16 * 1024 * 1024;
const RESOURCE_MIME: &str = "text/html;profile=mcp-app";
const CSP_FLAGS: [&str; 2] = ["unsafeEval", "wasmUnsafeEval"];
const PERMISSIONS: [&str; 4] = ["camera", "microphone", "geolocation", "clipboardWrite"];

pub(crate) struct Picked {
    pub(crate) html: String,
    pub(crate) meta: Option<Value>,
}

/// 选中 HTML 项：同 uri 优先；blob、超限一律拒绝，不降级、不截断。
pub(crate) fn pick_html(result: &Value, uri: &str) -> Result<Picked, String> {
    let contents = result["contents"]
        .as_array()
        .filter(|contents| !contents.is_empty())
        .ok_or_else(|| format!("MCP resource has no contents: {uri}"))?;
    let is_html = |content: &Value| content["mimeType"].as_str().is_some_and(html_mime);
    let content = contents
        .iter()
        .find(|content| content["uri"].as_str() == Some(uri) && is_html(content))
        .or_else(|| contents.iter().find(|content| is_html(content)))
        .ok_or_else(|| {
            let mime = contents[0]["mimeType"].as_str().unwrap_or("(missing)");
            format!("MCP resource mimeType is not HTML: {mime}")
        })?;
    let html = content["text"]
        .as_str()
        .ok_or_else(|| "MCP resource must return HTML as text, blob is not accepted".to_owned())?;
    if html.len() > HTML_MAX_BYTES {
        return Err(format!("MCP resource exceeds {HTML_MAX_BYTES} bytes"));
    }
    Ok(Picked {
        html: html.to_owned(),
        meta: normalize_resource_meta(&content["_meta"]),
    })
}

/// `text/html;profile=mcp-app` 或 `text/html`（忽略 charset 等次要参数）。
fn html_mime(value: &str) -> bool {
    let normalized = value
        .split(';')
        .map(|part| part.trim().to_lowercase())
        .filter(|part| !part.is_empty() && !part.starts_with("charset="))
        .collect::<Vec<_>>()
        .join(";");
    normalized == RESOURCE_MIME || normalized == "text/html"
}

/// `_meta` → `{csp, prefersBorder, heightHint, minFrameHeight, showInline, cspRelaxations, permissions}`；
/// 什么都没声明返回 None。
pub(crate) fn normalize_resource_meta(meta: &Value) -> Option<Value> {
    let root = meta.as_object()?;
    let ui = root.get("ui").and_then(Value::as_object);
    let mut result = Map::new();
    if let Some(csp) = read_csp(
        ui.and_then(|ui| ui.get("csp")),
        root.get("openai/widgetCSP"),
    ) {
        result.insert("csp".into(), csp);
    }
    if let Some(prefers) = read_bool(ui.and_then(|ui| ui.get("prefersBorder")))
        .or_else(|| read_bool(root.get("openai/widgetPrefersBorder")))
    {
        result.insert("prefersBorder".into(), Value::Bool(prefers));
    }
    if let Some(height) = read_positive_int(root.get("openai/widgetHeightHint")) {
        result.insert("heightHint".into(), json!(height));
    }
    if let Some(height) = read_positive_int(root.get("openai/widgetMinFrameHeight")) {
        result.insert("minFrameHeight".into(), json!(height));
    }
    if let Some(show) = read_bool(ui.and_then(|ui| ui.get("showInline")))
        .or_else(|| read_bool(root.get("openai/widgetShowCodexWidgetInline")))
    {
        result.insert("showInline".into(), Value::Bool(show));
    }
    if let Some(relaxations) = read_relaxations(root.get("escode/csp")) {
        result.insert("cspRelaxations".into(), relaxations);
    }
    let permissions: Vec<Value> = ui
        .and_then(|ui| ui.get("permissions"))
        .and_then(Value::as_object)
        .map(|permissions| {
            PERMISSIONS
                .iter()
                .filter(|key| permissions.get(**key).is_some_and(Value::is_object))
                .map(|key| json!(key))
                .collect()
        })
        .unwrap_or_default();
    if !permissions.is_empty() {
        result.insert("permissions".into(), Value::Array(permissions));
    }
    (!result.is_empty()).then_some(Value::Object(result))
}

fn read_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

fn read_positive_int(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.round() as i64)
}

fn read_relaxations(value: Option<&Value>) -> Option<Value> {
    let record = value?.as_object()?;
    let mut result = Map::new();
    for flag in CSP_FLAGS {
        if record.get(flag) == Some(&Value::Bool(true)) {
            result.insert(flag.into(), Value::Bool(true));
        }
    }
    (!result.is_empty()).then_some(Value::Object(result))
}

fn read_csp(modern: Option<&Value>, legacy: Option<&Value>) -> Option<Value> {
    let modern = modern.and_then(Value::as_object);
    let legacy = legacy.and_then(Value::as_object);
    let pick = |camel: &str, snake: &str| {
        read_origins(modern.and_then(|record| record.get(camel)))
            .or_else(|| read_origins(legacy.and_then(|record| record.get(camel))))
            .or_else(|| read_origins(legacy.and_then(|record| record.get(snake))))
    };
    let mut csp = Map::new();
    for (key, camel, snake) in [
        ("connectDomains", "connectDomains", "connect_domains"),
        ("resourceDomains", "resourceDomains", "resource_domains"),
        ("frameDomains", "frameDomains", "frame_domains"),
        ("baseUriDomains", "baseUriDomains", "base_uri_domains"),
    ] {
        if let Some(origins) = pick(camel, snake) {
            csp.insert(key.into(), Value::Array(origins));
        }
    }
    (!csp.is_empty()).then_some(Value::Object(csp))
}

fn read_origins(value: Option<&Value>) -> Option<Vec<Value>> {
    let list = value?.as_array()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut origins = Vec::new();
    for origin in list.iter().filter_map(|value| value.as_str()) {
        if let Some(origin) = normalize_origin(origin)
            && seen.insert(origin.clone())
        {
            origins.push(Value::from(origin));
        }
    }
    (!origins.is_empty()).then_some(origins)
}

/// `https:`/`wss:` 任意 host；`http:`/`ws:` 只放 `127.0.0.1`；拒绝凭据/query/hash/path。
fn normalize_origin(value: &str) -> Option<String> {
    let url = url::Url::parse(value.trim()).ok()?;
    let scheme = url.scheme();
    let host = url.host_str()?;
    let allowed = matches!(scheme, "https" | "wss")
        || (matches!(scheme, "http" | "ws") && host == "127.0.0.1");
    if !allowed
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return None;
    }
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Some(format!("{scheme}://{host}{port}"))
}

#[cfg(test)]
#[path = "plugin_ui_meta_tests.rs"]
mod tests;
