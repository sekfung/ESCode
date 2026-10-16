//! `escode-task.getModelTrajectory`（TS `escode-agent/modelTrajectory.ts` + `modelTrajectoryFileTail.ts`）：
//! 从 `<root>/.escode/cli/{debug,rollout}/model-io-<session>.jsonl` 还原一个 task 的模型调用轨迹。
//! docs/specs/rust-u2-tasks-index.md §7。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

#[path = "model_trajectory_map.rs"]
mod map;
use map::{as_object, map_record};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const DEFAULT_LIMIT: usize = 200;
/// 32 MiB 足以覆盖常规最近调用，同时避免大诊断文件造成内存峰值。
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;

/// TS `sanitizeSessionSegment`。
pub fn sanitize_session_segment(value: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            if pending {
                out.push('-');
                pending = false;
            }
            out.push(ch);
        } else {
            pending = true;
        }
    }
    if pending {
        out.push('-');
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(80).collect()
}

/// TS `resolveModelIODirs`：家目录与数据根（去重），各取 debug / rollout。
pub fn model_io_dirs(home: &Path, data_base_dir: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in [
        home.join(".escode").join("cli"),
        data_base_dir.join(".escode").join("cli"),
    ] {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
        .iter()
        .flat_map(|root| [root.join("debug"), root.join("rollout")])
        .collect()
}

struct Tail {
    text: String,
    truncated: bool,
}

async fn read_tail(path: &Path) -> std::io::Result<Tail> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let start = size.saturating_sub(MAX_READ_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buffer = Vec::with_capacity(usize::try_from(size - start).unwrap_or(0));
    file.take(size - start).read_to_end(&mut buffer).await?;
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if start > 0 {
        text = match text.find('\n') {
            Some(index) => text[index + 1..].to_owned(),
            None => String::new(),
        };
    }
    Ok(Tail {
        text,
        truncated: start > 0,
    })
}

/// `Date.parse` 的 ISO 8601 子集；其它格式按 NaN→0 处理（登记差异）。
fn to_time(value: Option<&str>) -> i64 {
    let Some(text) = value else { return 0 };
    parse_iso(text).unwrap_or(0)
}

pub(super) fn parse_iso(text: &str) -> Option<i64> {
    let (date, rest) = match text.split_once('T') {
        Some((date, rest)) => (date, Some(rest)),
        None => (text, None),
    };
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next().map_or(Some(1), |m| m.parse().ok())?;
    let day: i64 = parts.next().map_or(Some(1), |d| d.parse().ok())?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (mut hour, mut minute, mut second, mut millis, mut offset_minutes) = (0, 0, 0, 0, 0);
    if let Some(rest) = rest {
        let (clock, zone) = match rest.find(['Z', '+', '-']) {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "Z"),
        };
        let mut clock_parts = clock.split(':');
        hour = clock_parts.next()?.parse().ok()?;
        minute = clock_parts.next().map_or(Some(0), |m| m.parse().ok())?;
        if let Some(seconds) = clock_parts.next() {
            let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
            second = whole.parse().ok()?;
            if !fraction.is_empty() {
                let digits: String = fraction.chars().take(3).collect();
                millis = format!("{digits:0<3}").parse().ok()?;
            }
        }
        if zone != "Z" {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let body = &zone[1..];
            let (zh, zm) = body.split_once(':').unwrap_or((body, "0"));
            offset_minutes = sign * (zh.parse::<i64>().ok()? * 60 + zm.parse::<i64>().ok()?);
        }
    }
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute - offset_minutes) * 60_000 + second * 1000 + millis)
}

/// civil → 天数（Howard Hinnant）。
pub(super) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// TS `expandMessageCollection`：delta 记录用上一条还原完整上下文。
fn expand_collection(
    target: &mut Map<String, Value>,
    previous: Option<&Map<String, Value>>,
    keys: (&str, &str, &str),
    metadata: &Map<String, Value>,
) {
    let (collection, kind, offset) = keys;
    if metadata.get(kind).and_then(Value::as_str) != Some("delta") {
        return;
    }
    let delta = target.get(collection).and_then(Value::as_array).cloned();
    let before = previous
        .and_then(|p| p.get(collection))
        .and_then(Value::as_array);
    let offset = metadata.get(offset).and_then(Value::as_u64);
    let (Some(delta), Some(before), Some(offset)) = (delta, before, offset) else {
        return;
    };
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let mut merged: Vec<Value> = before.iter().take(offset).cloned().collect();
    merged.extend(delta);
    target.insert(collection.to_owned(), Value::Array(merged));
}

fn expand_records(records: &[Map<String, Value>]) -> Vec<Map<String, Value>> {
    let mut expanded: Vec<Map<String, Value>> = Vec::new();
    for record in records {
        let mut next = record.clone();
        if let Some(request) = as_object(record.get("request")) {
            let previous_request = expanded
                .last()
                .and_then(|p| as_object(p.get("request")))
                .cloned();
            let mut request = request.clone();
            let metadata = request.clone();
            expand_collection(
                &mut request,
                previous_request.as_ref(),
                ("messages", "messagesKind", "messageOffset"),
                &metadata,
            );
            expand_collection(
                &mut request,
                previous_request.as_ref(),
                ("sdkMessages", "sdkMessagesKind", "sdkMessageOffset"),
                &metadata,
            );
            if let Some(Value::Object(body)) = request.get("body").cloned() {
                let mut body = body;
                let previous_body = previous_request
                    .as_ref()
                    .and_then(|p| as_object(p.get("body")))
                    .cloned();
                expand_collection(
                    &mut body,
                    previous_body.as_ref(),
                    ("messages", "bodyMessagesKind", "bodyMessageOffset"),
                    &metadata,
                );
                request.insert("body".into(), Value::Object(body));
            }
            next.insert("request".into(), Value::Object(request));
        }
        expanded.push(next);
    }
    expanded
}

/// TS `readModelTrajectory`。`compare` 是 `localeCompare`（requestId 平手时的兜底键）。
pub async fn read_model_trajectory(
    dirs: &[PathBuf],
    task_id: &str,
    limit: Option<f64>,
    compare: &(dyn Fn(&str, &str) -> std::cmp::Ordering + Sync),
) -> Value {
    let limit = limit
        .filter(|l| l.is_finite() && *l > 0.0)
        .map_or(DEFAULT_LIMIT, |l| l.trunc() as usize);
    let sanitized = sanitize_session_segment(task_id);
    let file_name = format!(
        "model-io-{}.jsonl",
        if sanitized.is_empty() {
            "no-session"
        } else {
            &sanitized
        }
    );
    let mut source_files: Vec<String> = Vec::new();
    let mut raw: Vec<Map<String, Value>> = Vec::new();
    let mut input_truncated = false;
    let mut seen = BTreeSet::new();
    for dir in dirs {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        let mut present = false;
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_string_lossy() == file_name {
                present = true;
            }
        }
        if !present {
            continue;
        }
        let path = dir.join(&file_name);
        let Ok(tail) = read_tail(&path).await else {
            continue;
        };
        input_truncated |= tail.truncated;
        let mut matched = false;
        for line in tail.text.split('\n') {
            let trimmed = crate::git::paths::js_trim(line);
            if trimmed.is_empty() {
                continue;
            }
            let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if parsed.get("type").and_then(Value::as_str) != Some("model_io")
                || parsed.get("sessionId").and_then(Value::as_str) != Some(task_id)
            {
                continue;
            }
            raw.push(parsed);
            matched = true;
        }
        if matched {
            source_files.push(path.to_string_lossy().into_owned());
        }
    }
    raw.sort_by(|left, right| {
        let start = to_time(left.get("startedAt").and_then(Value::as_str))
            .cmp(&to_time(right.get("startedAt").and_then(Value::as_str)));
        start.then_with(|| {
            compare(
                left.get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                right
                    .get("requestId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        })
    });
    let records: Vec<Value> = expand_records(&raw).iter().map(map_record).collect();
    let truncated = input_truncated || records.len() > limit;
    // TS `records.slice(records.length - limit)`：差为负数时 JS 按“从末尾数”解释（输入被截断但条数不足上限时会丢前面的记录，原样复刻）。
    let kept = if truncated {
        let start = i64::try_from(records.len()).unwrap_or(0) - i64::try_from(limit).unwrap_or(0);
        let index = if start < 0 {
            (i64::try_from(records.len()).unwrap_or(0) + start).max(0)
        } else {
            start
        };
        records[usize::try_from(index).unwrap_or(0)..].to_vec()
    } else {
        records
    };
    json!({"taskId": task_id, "available": true, "records": kept, "sourceFiles": source_files, "truncated": truncated})
}

#[cfg(test)]
#[path = "model_trajectory_tests.rs"]
mod tests;
