//! MCP 工具卡的 UI 描述投影（对齐 TS `normalizeMcpToolUiMeta` + `createMcpToolDisplayUi`）。
//!
//! 输入是 `tools/list` 的原始 `_meta` 与 `tools/call` 的原始结果；输出是行级 `display.ui`。
//! UI 的 `pluginUiRowPolicy` 要求 `display.ui !== undefined` 才建卡，缺这份投影时 MCP App
//! 卡片在 Rust runtime 下完全不出现（含后台创建入口的判定前置）。
use serde_json::{Map, Value, json};
use url::Url;

const RESOURCE_URI_SCHEME: &str = "ui://";
const MAX_RESOURCE_URI_CHARS: usize = 2048;
const MAX_SURFACE_ID_CHARS: usize = 128;
const MAX_STRUCTURED_CONTENT_BYTES: usize = 64 * 1024;
const MAX_WIDGET_META_BYTES: usize = 16 * 1024;
const MAX_CONTENT_BYTES: usize = 32 * 1024;
const MAX_NAME_CHARS: usize = 256;

/// `_meta` → `McpToolUiDescriptor`。resourceUri 非法即整体为 None（可见性另有入口）。
pub fn normalize_meta(meta: &Value) -> Option<Value> {
    let ui = meta.get("ui").and_then(Value::as_object);
    let resource_uri = ui
        .and_then(|ui| read_resource_uri(&ui["resourceUri"]))
        .or_else(|| read_resource_uri(&meta["ui/resourceUri"]))
        .or_else(|| read_resource_uri(&meta["openai/outputTemplate"]))?;
    let mut descriptor = Map::new();
    descriptor.insert("resourceUri".into(), resource_uri.into());
    descriptor.insert(
        "visibility".into(),
        visibility(ui.and_then(|ui| ui.get("visibility"))),
    );
    let preferred = read_display_mode(&meta["openai/ui"]["preferredModelDisplayMode"]);
    if let Some(mode) = preferred {
        descriptor.insert("preferredDisplayMode".into(), mode.into());
    }
    // 修复：现代 CSP 在 `_meta.ui.csp`（TS `readCsp(asRecord(ui?.csp), ...)`）。这里过去把 `ui`
    // 本身当成 CSP 容器，`connectDomains` 永远读不到，只有 `openai/widgetCSP` 别名生效——
    // 声明在规范位置的工具在 Rust runtime 下丢失 CSP 放行（差分语料 `csp-modern` / `csp-projected`）。
    let modern_csp = ui.and_then(|ui| ui.get("csp")).and_then(Value::as_object);
    if let Some(csp) = read_csp(modern_csp, meta["openai/widgetCSP"].as_object()) {
        descriptor.insert("csp".into(), csp);
    }
    let prefers_border = read_bool(ui.and_then(|ui| ui.get("prefersBorder")))
        .or_else(|| read_bool(Some(&meta["openai/widgetPrefersBorder"])));
    if let Some(value) = prefers_border {
        descriptor.insert("prefersBorder".into(), value.into());
    }
    if let Some(surface) = read_surface_id(ui.and_then(|ui| ui.get("surface"))) {
        descriptor.insert("surface".into(), surface.into());
    }
    let show_inline = read_bool(ui.and_then(|ui| ui.get("showInline")))
        .or_else(|| read_bool(Some(&meta["openai/widgetShowCodexWidgetInline"])));
    if let Some(value) = show_inline {
        descriptor.insert("showInline".into(), value.into());
    }
    Some(Value::Object(descriptor))
}

/// 行级 `display.ui`：描述符 + 本次结果的结构化数据（超限整段省略并记账，不产生半截 JSON）。
pub fn display_ui(descriptor: &Value, plugin_id: &str, output: Option<&Value>) -> Option<Value> {
    let plugin_id = bound_name(plugin_id, MAX_NAME_CHARS)?;
    let resource_uri = read_resource_uri(&descriptor["resourceUri"])?;
    let mut ui = Map::new();
    ui.insert("pluginId".into(), plugin_id.into());
    ui.insert("resourceUri".into(), resource_uri.into());
    for key in ["preferredDisplayMode", "prefersBorder", "csp"] {
        if !descriptor[key].is_null() {
            ui.insert(key.into(), descriptor[key].clone());
        }
    }
    let record = output.filter(|value| value.is_object());
    let structured =
        record.and_then(|r| bound_json(&r["structuredContent"], MAX_STRUCTURED_CONTENT_BYTES));
    let widget_meta = record.and_then(|r| bound_json(&r["_meta"], MAX_WIDGET_META_BYTES));
    let content = record
        .and_then(|r| r["content"].is_array().then(|| &r["content"]))
        .and_then(|value| bound_json(value, MAX_CONTENT_BYTES));
    let mut truncated_bytes = 0usize;
    for (key, bound) in [
        ("structuredContent", &structured),
        ("widgetMeta", &widget_meta),
        ("content", &content),
    ] {
        match bound {
            Some(Bounded::Text(text)) => {
                ui.insert(key.into(), text.as_str().into());
            }
            Some(Bounded::Truncated(bytes)) => truncated_bytes += bytes,
            None => {}
        }
    }
    if truncated_bytes > 0 {
        ui.insert("truncated".into(), true.into());
        ui.insert("truncatedBytes".into(), truncated_bytes.into());
    }
    // H06：MCP isError 的调用在 v4 行上仍是 success（模型视角已完成），页面靠这个字段区分。
    if record.is_some_and(|r| r["isError"] == true) {
        ui.insert("isError".into(), true.into());
    }
    if let Some(surface) = read_surface_id(descriptor.get("surface")) {
        ui.insert("surface".into(), surface.into());
    }
    if descriptor["showInline"] == true {
        ui.insert("showInline".into(), true.into());
    }
    Some(Value::Object(ui))
}

/// live 工具结果 → `v4/plugin-ui/tool-result` 参数（`pluginUiToolResultNotificationSchema` 的 strict 形状）。
///
/// 只在成功后投递：历史 hydration 不经过 `apply_event`，失败/取消/MCP isError 由这里拦掉，
/// 因此 App 的 `attachPluginUiToolResults` 只会因真实成功结果创建页面。
pub fn tool_result_notification(session_id: &str, row: &Value) -> Option<Value> {
    if row["kind"] != "toolCall" || row["status"] != "success" {
        return None;
    }
    let display = &row["display"];
    if display["kind"] != "mcp_tool" || display["ui"]["isError"] == true {
        return None;
    }
    let tool_call_id = row["toolCallId"].as_str()?.trim();
    let server_name = display["serverName"].as_str()?.trim();
    let ui = display["ui"].as_object()?;
    if tool_call_id.is_empty() || server_name.is_empty() || ui.is_empty() {
        return None;
    }
    let mut params = Map::new();
    params.insert("sessionId".into(), session_id.into());
    params.insert("toolCallId".into(), tool_call_id.into());
    params.insert("serverName".into(), server_name.into());
    params.insert("ui".into(), display["ui"].clone());
    if row["input"].is_object() {
        params.insert("input".into(), row["input"].clone());
    }
    Some(Value::Object(params))
}

/// 插件 server 名为 `plugin:<pluginId>:<server>`（crates/plugins/src/mcp_config.rs）；其余用 server 名顶替。
pub fn plugin_id_for_server(server: &str) -> &str {
    server
        .strip_prefix("plugin:")
        .and_then(|rest| rest.split(':').next())
        .filter(|id| !id.is_empty())
        .unwrap_or(server)
}

enum Bounded {
    Text(String),
    Truncated(usize),
}

fn bound_json(value: &Value, max_bytes: usize) -> Option<Bounded> {
    if value.is_null() {
        return None;
    }
    let text = serde_json::to_string(value).ok()?;
    Some(if text.len() <= max_bytes {
        Bounded::Text(text)
    } else {
        Bounded::Truncated(text.len())
    })
}

fn read_resource_uri(value: &Value) -> Option<String> {
    let trimmed = value.as_str()?.trim();
    (trimmed.starts_with(RESOURCE_URI_SCHEME) && trimmed.chars().count() <= MAX_RESOURCE_URI_CHARS)
        .then(|| trimmed.to_owned())
}

fn visibility(value: Option<&Value>) -> Value {
    let filtered: Vec<Value> = value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| matches!(item.as_str(), Some("model" | "app")))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut unique: Vec<Value> = Vec::new();
    for item in filtered {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    if unique.is_empty() {
        json!(["model", "app"])
    } else {
        Value::Array(unique)
    }
}

fn read_display_mode(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "inline" => Some("inline"),
        "fullscreen" => Some("fullscreen"),
        "pip" => Some("pip"),
        _ => None,
    }
}

fn read_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

fn read_surface_id(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_SURFACE_ID_CHARS {
        return None;
    }
    let mut chars = trimmed.chars();
    let head = chars.next()?;
    (head.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    .then(|| trimmed.to_owned())
}

fn read_csp(
    modern: Option<&Map<String, Value>>,
    legacy: Option<&Map<String, Value>>,
) -> Option<Value> {
    let pick = |camel: &str, snake: &str| {
        read_origin_list(modern.and_then(|modern| modern.get(camel)))
            .or_else(|| read_origin_list(legacy.and_then(|legacy| legacy.get(camel))))
            .or_else(|| read_origin_list(legacy.and_then(|legacy| legacy.get(snake))))
    };
    let mut csp = Map::new();
    for (key, (camel, snake)) in [
        ("connectDomains", ("connectDomains", "connect_domains")),
        ("resourceDomains", ("resourceDomains", "resource_domains")),
        ("frameDomains", ("frameDomains", "frame_domains")),
        ("baseUriDomains", ("baseUriDomains", "base_uri_domains")),
    ] {
        if let Some(origins) = pick(camel, snake) {
            csp.insert(
                key.into(),
                Value::Array(origins.into_iter().map(Value::from).collect()),
            );
        }
    }
    (!csp.is_empty()).then_some(Value::Object(csp))
}

/// 只放行 `https:`/`wss:` 任意 host 与 `http:`/`ws:` 的 127.0.0.1；凭据、query、hash、路径一律拒绝。
fn read_origin_list(value: Option<&Value>) -> Option<Vec<String>> {
    let items = value?.as_array()?;
    let mut origins: Vec<String> = Vec::new();
    for item in items {
        let Some(origin) = normalize_origin(item) else {
            continue;
        };
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    // 修复：TS `readOriginList` 只要值本身是数组就返回（可空），空数组因此会在 CSP 里保留成
    // `frameDomains: []`；这里过去把空列表当成「没声明」丢掉，同一工具在 Rust/Node 下的
    // `display.ui.csp` 不一致（差分语料 `csp-legacy-camel-and-snake`）。
    Some(origins)
}

fn normalize_origin(value: &Value) -> Option<String> {
    let url = Url::parse(value.as_str()?.trim()).ok()?;
    let loopback = url.host_str() == Some("127.0.0.1") && matches!(url.scheme(), "http" | "ws");
    if !matches!(url.scheme(), "https" | "wss") && !loopback {
        return None;
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

fn bound_text(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed.chars().count() <= max_chars).then(|| trimmed.to_owned())
}

/// 展示名限长：超长按字符截断，与 TS `boundMcpDisplayText`（`trimmed.slice(0, maxChars)`）一致。
///
/// 修复：pluginId 过去复用 `bound_text`，超长时整体返回 None，`display.ui` 缺失使
/// `pluginUiRowPolicy` 不建卡；同一工具在 Node runtime 下会截断成 256 字符并正常建卡，
/// 形成 Rust/TS 差分（语料 `long-plugin-id-truncated`）。TS 的 slice 按 UTF-16 码元截断，
/// Rust 按字符截断；本仓库 pluginId 来自 ASCII 插件 id，两者一致。
fn bound_name(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(max_chars).collect())
}

#[cfg(test)]
#[path = "mcp_tool_ui_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mcp_tool_ui_corpus_tests.rs"]
mod corpus_tests;
