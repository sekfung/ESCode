//! `model` 字段的解析（TS `subagent-markdown-selection.ts` 的解析侧 + `custom-model-value.ts`）。
//!
//! 只做「markdown 里的字符串 → ModelSelection」；序列化与 legacy provider id 迁移属写面，不在本片。

use serde_json::{Value, json};

/// TS `decodeCustomModelValue`：`custom:<provider>[:<model>]`，`builtin:a:b:c` 是 legacy 三段式。
pub(super) fn decode_custom_model_value(value: &str) -> Option<(String, Option<String>)> {
    let body = value.strip_prefix("custom:")?;
    let Some(separator) = body.find(':') else {
        return Some((safe_decode(body), None));
    };
    let legacy: Vec<&str> = body.split(':').collect();
    if legacy.len() >= 3 && legacy[0] == "builtin" {
        return Some((
            format!("{}:{}", legacy[0], legacy[1]),
            Some(safe_decode(&legacy[2..].join(":"))),
        ));
    }
    Some((
        safe_decode(&body[..separator]),
        Some(safe_decode(&body[separator + 1..])),
    ))
}

fn safe_decode(value: &str) -> String {
    percent_decode(value).unwrap_or_else(|| value.to_owned())
}

/// `decodeURIComponent` 的最小等价：非法百分号转义保持原样（TS 的 `safeDecodeUriComponent`）。
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            // `decodeURIComponent` 对非法转义抛错，TS 侧吞掉后保留原文。
            let hex = value.get(index + 1..index + 3)?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).ok()
}

/// TS `parseModelPickerValue`：`provider/model[$reasoning]`；`$` 在首位或末位时不当作档位。
pub(super) fn parse_model_picker_value(value: &str) -> Option<Value> {
    let normalized = value.trim();
    let provider_end = normalized.find('/')?;
    if provider_end == 0 {
        return None;
    }
    let provider_id = &normalized[..provider_end];
    let raw_model = &normalized[provider_end + 1..];
    if provider_id.trim().is_empty() || raw_model.trim().is_empty() {
        return None;
    }
    let reasoning = raw_model.find('$');
    let (model_id, level) = match reasoning {
        Some(index) if index > 0 && index < raw_model.len() - 1 => {
            (&raw_model[..index], Some(&raw_model[index + 1..]))
        }
        _ => (raw_model, None),
    };
    if model_id.trim().is_empty() {
        return None;
    }
    let mut selection = json!({"providerId": provider_id, "modelId": model_id});
    if let Some(level) = level.map(str::trim).filter(|level| !level.is_empty()) {
        selection["options"] = json!({"reasoningLevel": level});
    }
    Some(selection)
}
