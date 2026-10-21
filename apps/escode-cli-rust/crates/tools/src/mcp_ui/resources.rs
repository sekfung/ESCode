//! 页面资源读取的边界（docs/specs/rust-mcp-ui.md §3）。
//!
//! 总大小按 text 的 UTF-8 字节 + blob 的 base64 解码字节累计，超过 8 MiB 直接拒绝（不截断）；
//! mimeType 必须在白名单，blob 缺 mimeType 视为不可判定。面向页面的结果去掉 `_meta`。
use anyhow::{Result, bail};
use base64::Engine as _;
use serde_json::{Map, Value, json};

pub(crate) const UI_READ_MAX_BYTES: usize = 8 * 1024 * 1024;

/// 白名单项以 `/` 结尾表示前缀匹配；否则忽略参数与大小写做精确匹配（TS `isMcpAppsUiReadResourceMimeAllowed`）。
const MIME_ALLOWLIST: &[&str] = &[
    "text/",
    "application/json",
    "application/xml",
    "application/yaml",
    "application/x-yaml",
    "application/pdf",
    "application/rtf",
    "application/msword",
    "application/vnd.ms-excel",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/svg+xml",
    "image/gif",
    "image/avif",
    "image/bmp",
    "image/tiff",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "image/heic",
    "image/heif",
    "audio/mpeg",
    "audio/mp4",
    "audio/ogg",
    "audio/wav",
    "audio/x-wav",
    "audio/webm",
    "audio/aac",
    "audio/flac",
    "video/mp4",
    "video/webm",
    "video/ogg",
    "video/quicktime",
    "video/mpeg",
    "application/zip",
    "application/gzip",
    "application/x-gzip",
    "application/x-tar",
    "application/x-7z-compressed",
    "application/vnd.rar",
    "font/woff",
    "font/woff2",
    "font/ttf",
    "font/otf",
    "model/gltf+json",
    "model/gltf-binary",
    "application/octet-stream",
];

pub(crate) fn mime_allowed(mime_type: &str) -> bool {
    let essence = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if essence.is_empty() {
        return false;
    }
    MIME_ALLOWLIST.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix('/') {
            essence.starts_with(&format!("{prefix}/"))
        } else {
            essence == *entry
        }
    })
}

/// `resources/read` 的原始结果 → 页面形状：去 `_meta`、校验大小与 MIME。
pub(crate) fn bound_for_ui(uri: &str, raw: &Value) -> Result<Value> {
    let contents = raw["contents"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut bytes = 0usize;
    let mut bounded = Vec::with_capacity(contents.len());
    for content in contents {
        let item = content.as_object().cloned().unwrap_or_default();
        let mime = item.get("mimeType").and_then(Value::as_str);
        if let Some(mime) = mime
            && !mime_allowed(mime)
        {
            bail!(
                "MCP resource \"{}\" has mimeType \"{mime}\" which is not allowed for plugin UI reads",
                item.get("uri").and_then(Value::as_str).unwrap_or(uri)
            );
        }
        let blob = item.get("blob").and_then(Value::as_str);
        if blob.is_some() && mime.is_none() {
            bail!(
                "MCP resource \"{}\" has mimeType \"(missing)\" which is not allowed for plugin UI reads",
                item.get("uri").and_then(Value::as_str).unwrap_or(uri)
            );
        }
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            bytes += text.len();
        }
        if let Some(blob) = blob {
            bytes += base64::engine::general_purpose::STANDARD
                .decode(blob.trim())
                .map(|decoded| decoded.len())
                .unwrap_or_else(|_| blob.len() / 4 * 3);
        }
        if bytes > UI_READ_MAX_BYTES {
            bail!(
                "MCP resource \"{uri}\" is {bytes} bytes, exceeding the {UI_READ_MAX_BYTES} byte limit for plugin UI reads"
            );
        }
        let mut out = Map::new();
        out.insert(
            "uri".into(),
            item.get("uri").cloned().unwrap_or_else(|| json!(uri)),
        );
        for key in ["mimeType", "text", "blob"] {
            if let Some(value) = item.get(key) {
                out.insert(key.into(), value.clone());
            }
        }
        bounded.push(Value::Object(out));
    }
    Ok(json!({ "contents": bounded }))
}

/// `resources/list` 结果只保留页面需要的标准描述符（保留 `_meta`）。
pub(crate) fn project_resources(raw: &Value) -> Value {
    let resources: Vec<Value> = raw["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|resource| {
            let mut out = Map::new();
            for key in ["uri", "name", "title", "description", "mimeType", "_meta"] {
                if let Some(value) = resource.get(key) {
                    out.insert(key.into(), value.clone());
                }
            }
            Value::Object(out)
        })
        .collect();
    let mut result = json!({ "resources": resources });
    if let Some(cursor) = raw["nextCursor"].as_str() {
        result["nextCursor"] = json!(cursor);
    }
    result
}

/// `resources/templates/list` 结果只保留标准模板字段。
pub(crate) fn project_templates(raw: &Value) -> Value {
    let templates: Vec<Value> = raw["resourceTemplates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|template| {
            let mut out = Map::new();
            for key in ["uriTemplate", "name", "title", "description", "mimeType"] {
                if let Some(value) = template.get(key) {
                    out.insert(key.into(), value.clone());
                }
            }
            Value::Object(out)
        })
        .collect();
    let mut result = json!({ "resourceTemplates": templates });
    if let Some(cursor) = raw["nextCursor"].as_str() {
        result["nextCursor"] = json!(cursor);
    }
    result
}
