//! `file` 域的预览读取（TS `readMediaPreview` / `readBinaryPreview`）与媒体类型判定。
//! 从 `domains_file.rs` 拆出以守住单文件行数上限。

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use crate::domains_file::{node_error, require_path};
use crate::session::ChannelResult;
use crate::wire::Wire;

/// TS `DEFAULT_MEDIA_PREVIEW_BYTES` / `MAX_MEDIA_PREVIEW_BYTES`。
pub(crate) const DEFAULT_MEDIA_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const MAX_MEDIA_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
/// TS `DEFAULT_BINARY_PREVIEW_BYTES` / `MAX_BINARY_PREVIEW_BYTES`（都是 25 MiB）。
pub(crate) const BINARY_PREVIEW_BYTES: u64 = 25 * 1024 * 1024;

/// TS `IMAGE_EXTENSION_TO_MEDIA_TYPE`（按 `path.extname().toLowerCase()` 查表）。
pub(crate) const IMAGE_EXTENSION_TO_MEDIA_TYPE: [(&str, &str); 10] = [
    (".apng", "image/apng"),
    (".avif", "image/avif"),
    (".bmp", "image/bmp"),
    (".gif", "image/gif"),
    (".ico", "image/x-icon"),
    (".jpeg", "image/jpeg"),
    (".jpg", "image/jpeg"),
    (".png", "image/png"),
    (".svg", "image/svg+xml"),
    (".webp", "image/webp"),
];

/// TS `MEDIA_PREVIEW_FORMATS`（`@escode/shared`：按下标顺序做 `endsWith` 匹配）。
pub(crate) const MEDIA_PREVIEW_FORMATS: [(&str, &str); 11] = [
    (".mp4", "video/mp4"),
    (".mov", "video/quicktime"),
    (".webm", "video/webm"),
    (".m4v", "video/x-m4v"),
    (".mp3", "audio/mpeg"),
    (".wav", "audio/wav"),
    (".m4a", "audio/mp4"),
    (".ogg", "audio/ogg"),
    (".opus", "audio/opus"),
    (".flac", "audio/flac"),
    (".weba", "audio/webm"),
];

/// TS `readMediaPreview`：整文件 base64 + 媒体类型；大小上限 clamp `[1, 8 MiB]`（缺省 4 MiB）。
pub(crate) fn read_media_preview(params: &Value) -> ChannelResult {
    let path = require_path(params)?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "stat", &path)))?;
    if !metadata.is_file() {
        return Err((
            "InvalidRequest".to_owned(),
            format!("Path is not a file: {path}"),
        ));
    }
    let max_bytes = clamp_preview_bytes(
        params["maxBytes"].as_f64(),
        DEFAULT_MEDIA_PREVIEW_BYTES,
        MAX_MEDIA_PREVIEW_BYTES,
    );
    if metadata.len() > max_bytes {
        return Err((
            "InvalidRequest".to_owned(),
            format!("File is too large to preview: {path}"),
        ));
    }
    let content = std::fs::read(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "open", &path)))?;
    Ok(Wire::Object(json!({
        "path": path,
        "mediaType": infer_media_type_from_path(&path),
        "dataBase64": BASE64.encode(&content),
        "totalBytes": metadata.len(),
    })))
}

/// TS `readBinaryPreview`：整文件 base64（Office 解析需要完整 ZIP/OLE 字节）；
/// 大小上限 clamp `[1, 25 MiB]`（缺省与上限同值）。
pub(crate) fn read_binary_preview(params: &Value) -> ChannelResult {
    let path = require_path(params)?;
    let metadata = std::fs::metadata(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "stat", &path)))?;
    if !metadata.is_file() {
        return Err((
            "InvalidRequest".to_owned(),
            format!("Path is not a file: {path}"),
        ));
    }
    let max_bytes = clamp_preview_bytes(
        params["maxBytes"].as_f64(),
        BINARY_PREVIEW_BYTES,
        BINARY_PREVIEW_BYTES,
    );
    if metadata.len() > max_bytes {
        return Err((
            "InvalidRequest".to_owned(),
            format!("File is too large to preview: {path}"),
        ));
    }
    let content = std::fs::read(&path)
        .map_err(|error| ("NodeIoError".to_owned(), node_error(&error, "open", &path)))?;
    Ok(Wire::Object(json!({
        "path": path,
        "dataBase64": BASE64.encode(&content),
        "totalBytes": metadata.len(),
    })))
}

/// TS `clampMediaPreviewBytes` / `clampBinaryPreviewBytes`：`trunc(maxBytes ?? 默认)` clamp 到 `[1, 上限]`。
pub(crate) fn clamp_preview_bytes(requested: Option<f64>, default: u64, max: u64) -> u64 {
    let value = requested
        .filter(|value| value.is_finite())
        .map_or(default, |value| value.trunc().max(0.0) as u64);
    value.clamp(1, max)
}

/// TS `inferMediaTypeFromPath`：图片扩展名表 → 共享媒体格式表（`endsWith`）→ `application/octet-stream`。
pub(crate) fn infer_media_type_from_path(path: &str) -> String {
    let extension = node_extname(path).to_lowercase();
    if let Some((_, media_type)) = IMAGE_EXTENSION_TO_MEDIA_TYPE
        .iter()
        .find(|(candidate, _)| *candidate == extension)
    {
        return (*media_type).to_owned();
    }
    let normalized = path.replace('\\', "/").to_lowercase();
    if let Some((_, media_type)) = MEDIA_PREVIEW_FORMATS
        .iter()
        .find(|(extension, _)| normalized.ends_with(extension))
    {
        return (*media_type).to_owned();
    }
    "application/octet-stream".to_owned()
}

/// Node `path.extname` 语义：basename 内最后一个 `.` 起（不含路径分隔符）；
/// basename 以 `.` 开头且无其它点时为 ""（`.bashrc` → ""），`a.` → "."。
pub(crate) fn node_extname(path: &str) -> String {
    let basename = path.rsplit(['/', '\\']).next().unwrap_or("");
    match basename[1..].rfind('.') {
        Some(index) => basename[index + 1..].to_owned(),
        None => String::new(),
    }
}
