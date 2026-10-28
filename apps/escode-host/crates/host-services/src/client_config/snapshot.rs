//! `client/configs` 信封 → 快照（TS `shared/clientConfig.ts: parseClientConfigSnapshot` + `pluginStoreOrder.ts`）。
//! 对拍见 `scripts/generate-escode-host-client-config-fixtures.mjs`。

use serde_json::{Map, Value, json};
use url::Url;

use crate::js_compat::js_trim;

/// `{pluginStoreOrder, zsrcUrl, uiPluginRecommendations}`（只允许显式接入的公开字段进入快照）。
#[derive(Debug, Clone, PartialEq)]
pub struct ClientConfigSnapshot {
    pub plugin_store_order: Option<Value>,
    pub zsrc_url: Option<String>,
    pub ui_plugin_recommendations: Vec<UiPluginRecommendation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPluginRecommendation {
    pub plugin_id: String,
    pub surface_id: String,
}

impl ClientConfigSnapshot {
    pub fn to_json(&self) -> Value {
        json!({
            "pluginStoreOrder": self.plugin_store_order.clone().unwrap_or(Value::Null),
            "zsrcUrl": self.zsrc_url.clone().map_or(Value::Null, Value::String),
            "uiPluginRecommendations": self
                .ui_plugin_recommendations
                .iter()
                .map(|item| json!({"pluginId": item.plugin_id, "surfaceId": item.surface_id}))
                .collect::<Vec<_>>(),
        })
    }
}

const INVALID: &str = "Invalid public client config response";

/// TS `parseClientConfigSnapshot`：信封 `{code: 0, data?: {configs?: {…}|null}|null}`，否则抛 `Invalid public client config response`。
pub fn parse_snapshot(payload: &Value) -> Result<ClientConfigSnapshot, String> {
    let Some(envelope) = payload.as_object() else {
        return Err(INVALID.to_owned());
    };
    if envelope.get("code").and_then(Value::as_f64) != Some(0.0) {
        return Err(INVALID.to_owned());
    }
    let configs = match envelope.get("data") {
        None | Some(Value::Null) => None,
        Some(Value::Object(data)) => match data.get("configs") {
            None | Some(Value::Null) => None,
            Some(Value::Object(configs)) => Some(configs),
            Some(_) => return Err(INVALID.to_owned()),
        },
        Some(_) => return Err(INVALID.to_owned()),
    };
    Ok(ClientConfigSnapshot {
        plugin_store_order: configs
            .and_then(|configs| configs.get("pluginStoreOrder"))
            .and_then(parse_plugin_store_order),
        zsrc_url: configs
            .and_then(|configs| configs.get("zsrc"))
            .and_then(parse_zsrc_url),
        ui_plugin_recommendations: configs
            .and_then(|configs| configs.get("uiPluginRecommendations"))
            .and_then(parse_ui_plugin_recommendations)
            .unwrap_or_default(),
    })
}

/// `z.array(z.object({pluginId, surfaceId}))` 的 `safeParse(...).data ?? []`：
/// 任一条目不合法则整体回退为空数组；合法时只保留 trim 后的两个字段（zod 丢弃未知键）。
fn parse_ui_plugin_recommendations(value: &Value) -> Option<Vec<UiPluginRecommendation>> {
    let mut parsed = Vec::new();
    for item in value.as_array()? {
        let item = item.as_object()?;
        let plugin_id = js_trim(item.get("pluginId")?.as_str()?);
        let surface_id = js_trim(item.get("surfaceId")?.as_str()?);
        if plugin_id.is_empty() || surface_id.is_empty() {
            return None;
        }
        parsed.push(UiPluginRecommendation {
            plugin_id: plugin_id.to_owned(),
            surface_id: surface_id.to_owned(),
        });
    }
    Some(parsed)
}

/// `{url: string}` 且 trim 后是 http(s) URL；否则 `None`。
fn parse_zsrc_url(value: &Value) -> Option<String> {
    let url = js_trim(value.as_object()?.get("url")?.as_str()?);
    let lower = url.to_ascii_lowercase();
    let http = lower.starts_with("http://") || lower.starts_with("https://");
    (http && Url::parse(url).is_ok()).then(|| url.to_owned())
}

/// TS `parsePluginStoreOrder`：宽容——整体不是对象 → `None`；`code`/`work` 各自独立，坏的回退为缺省。
fn parse_plugin_store_order(value: &Value) -> Option<Value> {
    let input = value.as_object()?;
    let mut output = Map::new();
    for mode in ["code", "work"] {
        if let Some(parsed) = input.get(mode).and_then(parse_mode_order) {
            output.insert(mode.to_owned(), parsed);
        }
    }
    Some(Value::Object(output))
}

/// `{categoryOrder?: string[], pluginOrder?: Record<string, string[]>}`：任一字段不合法则整个模式作废（`None`）。
fn parse_mode_order(value: &Value) -> Option<Value> {
    let input = value.as_object()?;
    let mut output = Map::new();
    if let Some(order) = input.get("categoryOrder") {
        output.insert("categoryOrder".to_owned(), parse_order_list(order)?);
    }
    if let Some(plugins) = input.get("pluginOrder") {
        let mut parsed = Map::new();
        for (key, list) in plugins.as_object()? {
            parsed.insert(key.clone(), parse_order_list(list)?);
        }
        output.insert("pluginOrder".to_owned(), Value::Object(parsed));
    }
    Some(Value::Object(output))
}

/// `z.array(z.string().trim().min(1)).transform(去重)`：条目 trim 后非空，输出 trim 后的值，保序去重。
fn parse_order_list(value: &Value) -> Option<Value> {
    let mut seen: Vec<String> = Vec::new();
    for item in value.as_array()? {
        let trimmed = js_trim(item.as_str()?);
        if trimmed.is_empty() {
            return None;
        }
        if !seen.iter().any(|existing| existing == trimmed) {
            seen.push(trimmed.to_owned());
        }
    }
    Some(Value::Array(seen.into_iter().map(Value::String).collect()))
}
