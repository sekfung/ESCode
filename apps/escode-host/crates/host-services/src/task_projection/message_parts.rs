//! `mapMessage` 的零件层：JSON 取值与 JS 语义判定、附件反投影、synthetic timeline 合成。
//!
//! 逐条对齐 `packages/services/src/escode-agent/escodeTaskServiceAdapter.ts` 的
//! `mapPromptAttachmentsFromParts` / `mapPromptAttachmentFromFilePart` /
//! `extractSyntheticTimelineFromTextPart` / `synthesizeCompactionTimeline` 与文件尾部小工具
//! （`asRecord` / `stringValue` / `numberValue` / `booleanValue` / `timelineStatusValue` /
//! `timelineTriggerValue` / `filenameFromPathLike` / `dataBase64FromDataUrl` /
//! `localPathFromAttachmentPart` / `isAbsolutePathLike`）。
//!
//! 从 `messages.rs` 拆出只为守住单文件 400 行上限：`messages.rs` 保留 `mapMessage` /
//! `mapToolPart` / `coalesceConsecutiveESCodeAssistants`，`message_merge.rs` 保留相邻 assistant 合并。

use serde_json::{Map, Value};

/// TS `part.mime || "application/octet-stream"` 的兜底值。
const DEFAULT_MIME: &str = "application/octet-stream";

/// TS 对象字面量的一项：`None` 等价于 TS 里值为 `undefined` 的键（JSON 里消失）。
pub(super) type Field = (&'static str, Option<Value>);

/// TS 对象字面量：键序即给定顺序。
pub(super) fn build_object(entries: impl IntoIterator<Item = Field>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        if let Some(value) = value {
            map.insert(key.to_owned(), value);
        }
    }
    Value::Object(map)
}

/// TS `asRecord(x).key`：非对象（含 `null`/数组）取不到字段。
pub(super) fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}

/// TS `stringValue`：非空字符串才算有值（不 trim）。
pub(super) fn string_value(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// JS 真值：`undefined`/`null`/`false`/`0`/`""` 为假，数组与对象恒真。
pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// 字段存在且 JS 真值（`!last.syntheticTimeline`、`part.auto` 这类判定）。
pub(super) fn truthy_field(value: &Value, key: &str) -> bool {
    truthy_field_value(value, key).is_some()
}

/// 字段存在且 JS 真值时的原值（`state.title || part.tool`、`info.model ? …` 这类）。
pub(super) fn truthy_field_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match field(value, key) {
        Some(candidate) if truthy(candidate) => Some(candidate),
        _ => None,
    }
}

/// `part.type`（TS `part.type === "file"` 这类判定）。
pub(super) fn array_field<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    field(value, key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub(super) fn part_type(part: &Value) -> Option<&str> {
    field(part, "type").and_then(Value::as_str)
}

/// `value.role`（只认字符串，对应 TS `role === "assistant"` 比较）。
pub(super) fn role_of(value: &Value) -> &str {
    field(value, "role").and_then(Value::as_str).unwrap_or("")
}

/// JS `===`：数字按数值比较（`1` 与 `1.0` 在 JS 里相等），其余按 JSON 结构比较。
pub(super) fn strict_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        _ => left == right,
    }
}

/// TS `next.goalIteration === last.goalIteration`（两侧都缺值即相等，`undefined !== null`）。
pub(super) fn strict_eq_optional(last: Option<&Value>, next: Option<&Value>) -> bool {
    match (last, next) {
        (Some(last), Some(next)) => strict_eq(last, next),
        (None, None) => true,
        _ => false,
    }
}

/// JS `String.prototype.trim` 的空白集合（含 BOM，不含 U+0085）。
pub(super) fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| {
        matches!(c, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}')
            || matches!(
                c,
                '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
            )
    })
}

/// JS 数字 → JSON：`NaN`/`Infinity` 按 `JSON.stringify` 变 `null`，整数值保持整数形态。
pub(super) fn js_number(value: f64) -> Value {
    if !value.is_finite() {
        return Value::Null;
    }
    let truncated = value.trunc();
    if truncated == value && truncated.abs() <= 9_007_199_254_740_992.0 {
        return Value::from(truncated as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// JS 算术里的数值化：缺值（`undefined`）为 `NaN`，`null`/布尔/数字字符串按 ToNumber 折算。
pub(super) fn arith_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(true)) => 1.0,
        Some(Value::Bool(false)) => 0.0,
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => text.trim().parse().unwrap_or(f64::NAN),
        Some(Value::Array(_) | Value::Object(_)) => f64::NAN,
    }
}

/// TS `numberValue(x) !== undefined` 时的原值透传（保住整数/小数形态）。
pub(super) fn number_clone(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(value) if value.is_number() => Some(value.clone()),
        _ => None,
    }
}

/// TS `formatModelPickerValue`：`provider/model`，有推理档时接 `$推理档`。
pub(super) fn format_model_selection(model: &Value) -> String {
    // 展示边界不替用户补默认模型（缺值同 JS 模板字符串里的 `undefined`）。
    let provider = field(model, "providerId")
        .and_then(Value::as_str)
        .unwrap_or("undefined");
    let model_id = field(model, "modelId")
        .and_then(Value::as_str)
        .unwrap_or("undefined");
    let base = format!("{provider}/{model_id}");
    let level = truthy_field_value(
        field(model, "options").unwrap_or(&Value::Null),
        "reasoningLevel",
    );
    match level.and_then(Value::as_str) {
        Some(level) => format!("{base}${level}"),
        None => base,
    }
}

/// TS `isAbsolutePathLike`：`/`、`X:\`/`X:/`、UNC `\\` 开头。
pub(super) fn is_absolute_path_like(value: &str) -> bool {
    if value.starts_with('/') || value.starts_with(r"\\") {
        return true;
    }
    let mut chars = value.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(drive), Some(':'), Some(separator))
            if drive.is_ascii_alphabetic() && matches!(separator, '\\' | '/')
    )
}

/// TS `filenameFromPathLike`：`data:` 不算，取最后一段并 trim，含 `://` 的不算文件名。
fn filename_from_path_like(value: &str) -> Option<String> {
    if value.starts_with("data:") {
        return None;
    }
    let path_part = value.split(['?', '#']).next().unwrap_or(value);
    let mut segments = path_part
        .split(['\\', '/'])
        .filter(|segment| !segment.is_empty());
    let name = js_trim(segments.next_back()?);
    (!name.is_empty() && !name.contains("://")).then(|| name.to_owned())
}

/// TS `dataBase64FromDataUrl`：`/^data:[^;,]+;base64,(.*)$/i`（JS `.` 不吃换行，带换行即不匹配）。
fn data_base64_from_data_url(value: &str) -> Option<String> {
    if !value.get(..5)?.eq_ignore_ascii_case("data:") {
        return None;
    }
    let rest = value.get(5..)?;
    let separator = rest.find([';', ','])?;
    let (head, tail) = rest.split_at(separator);
    // `[^;,]+` 要求第一个分隔符是 `;`（且前面至少一个字符），其后必须是 `base64,`。
    if head.is_empty() || !tail.starts_with(';') {
        return None;
    }
    let (marker, payload) = tail.get(1..)?.split_at_checked(7)?;
    if !marker.eq_ignore_ascii_case("base64,") {
        return None;
    }
    if payload.is_empty() || payload.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    Some(payload.to_owned())
}

/// TS `localPathFromAttachmentPart`：`metadata.originalUrl` 优先，其次 `url`，都要是绝对路径形态。
fn local_path_from_attachment_part(url: &str, metadata: &Value) -> Option<String> {
    if let Some(original) = string_value(field(metadata, "originalUrl"))
        && is_absolute_path_like(original)
    {
        return Some(original.to_owned());
    }
    is_absolute_path_like(url).then(|| url.to_owned())
}

/// TS `part.filename?.trim() || filenameFromPathLike(part.url) || "attachment"`。
fn attachment_filename(part: &Value) -> String {
    if let Some(name) = field(part, "filename").and_then(Value::as_str) {
        let trimmed = js_trim(name);
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    if let Some(url) = field(part, "url").and_then(Value::as_str)
        && let Some(name) = filename_from_path_like(url)
    {
        return name;
    }
    "attachment".to_owned()
}

/// TS `mapPromptAttachmentsFromParts`：只反投影 `file` part；空列表按 TS 给 `undefined`。
pub(super) fn map_prompt_attachments_from_parts(parts: &[Value]) -> Option<Vec<Value>> {
    let attachments: Vec<Value> = parts
        .iter()
        .filter(|part| part_type(part) == Some("file"))
        .filter_map(map_prompt_attachment_from_file_part)
        .collect();
    (!attachments.is_empty()).then_some(attachments)
}

/// TS `mapPromptAttachmentFromFilePart`：按 mime 前缀还原发送时的附件形态。
fn map_prompt_attachment_from_file_part(part: &Value) -> Option<Value> {
    let mime = string_value(field(part, "mime")).unwrap_or(DEFAULT_MIME);
    let metadata = field(part, "metadata").unwrap_or(&Value::Null);
    let url = field(part, "url").and_then(Value::as_str);
    let size_bytes = number_clone(field(metadata, "sizeBytes"));
    let data_base64 = url.and_then(data_base64_from_data_url).map(Value::from);
    let local_path = url
        .and_then(|url| local_path_from_attachment_part(url, metadata))
        .map(Value::from);
    let kind = if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime.starts_with("video/") {
        "video"
    } else {
        "file"
    };
    let mut fields: Vec<Field> = vec![
        ("kind", Some(Value::from(kind))),
        ("filename", Some(Value::from(attachment_filename(part)))),
        ("mimeType", Some(Value::from(mime))),
    ];
    // audio 没有 sizeBytes；image/video 是条件键；file 恒有（缺值补 0）。
    match kind {
        "audio" => {}
        "file" => fields.push(("sizeBytes", Some(size_bytes.unwrap_or(Value::from(0))))),
        _ => fields.push(("sizeBytes", size_bytes)),
    }
    fields.push(("dataBase64", data_base64));
    if kind == "file" {
        // 有本地路径时不再回填 preview 文本（TS `!localPath ? stringValue(preview.text) : undefined`）。
        let preview = field(metadata, "preview");
        let text = match local_path {
            None => string_value(field(preview.unwrap_or(&Value::Null), "text")).map(Value::from),
            Some(_) => None,
        };
        fields.push(("textContent", text));
    }
    fields.push(("localPath", local_path));
    Some(build_object(fields))
}

#[cfg(test)]
#[path = "message_parts_tests.rs"]
mod tests;
