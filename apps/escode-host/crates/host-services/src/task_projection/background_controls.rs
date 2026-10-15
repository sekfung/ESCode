//! 后台任务控制项（`packages/shared/src/background-task-controls.ts` 的
//! `parseESCodeBackgroundTaskControlItems`）：旧 background task 事件的 kind/命令/状态兼容解析，
//! 字段别名多，逐条对齐。
//!
//! 登记差异：`readInstantMs` 的字符串分支在 TS 走 `Date.parse`（能吃 RFC 2822 等格式），
//! 这里只认 ISO 8601 子集；差异只在「后台任务事件的时间字段是字符串而非毫秒数」时可见。

use std::collections::HashMap;

use serde_json::{Map, Value};

pub fn parse_control_items(value: &[Value]) -> Vec<Value> {
    let mut positions: HashMap<String, usize> = HashMap::new();
    let mut jobs: Vec<Value> = Vec::new();
    for item in value {
        let Some(parsed) = parse_control_item(item) else {
            continue;
        };
        let job_id = string_field(&parsed, &["jobId"]).unwrap_or_default();
        match positions.get(&job_id) {
            Some(index) => jobs[*index] = parsed,
            None => {
                positions.insert(job_id, jobs.len());
                jobs.push(parsed);
            }
        }
    }
    jobs
}
fn parse_control_item(item: &Value) -> Option<Value> {
    if !item.is_object() {
        return None;
    }
    let task_kind = resolve_task_kind(item)?;
    let command = read_display_command(item, task_kind)?;
    let job_id = read_job_id(item, &command);
    let tool_call_id = string_field(item, &["toolCallId", "tool_call_id"]);
    let started_at = read_instant_ms(
        item,
        &["startedAt", "started_at", "startTime", "start_time"],
    );
    let elapsed_ms = read_number_field(
        item,
        &["elapsedMs", "elapsed_ms", "durationMs", "duration_ms"],
    );
    let pid = read_number_field(item, &["pid", "processId", "process_id"]);
    let title = string_field(item, &["title", "name", "label", "description"]);
    let status = normalize_status(string_field(item, &["status", "state", "phase"]).as_deref());
    let cancellable = read_boolean_field(item, &["cancellable"]);
    let stdout_tail = string_field(item, &["stdoutTail", "stdout_tail"]);
    let stderr_tail = string_field(item, &["stderrTail", "stderr_tail"]);
    let output_tail = string_field(item, &["outputTail", "output_tail"]);

    let mut job = Map::new();
    job.insert("jobId".to_owned(), Value::from(job_id));
    insert_optional(&mut job, "toolCallId", tool_call_id);
    job.insert("command".to_owned(), Value::from(command));
    job.insert("taskKind".to_owned(), Value::from(task_kind));
    if let Some(cancellable) = cancellable {
        job.insert("cancellable".to_owned(), Value::Bool(cancellable));
    }
    insert_optional(&mut job, "title", title);
    job.insert("status".to_owned(), Value::from(status));
    insert_number(&mut job, "startedAt", started_at);
    insert_number(&mut job, "elapsedMs", elapsed_ms);
    insert_number(&mut job, "pid", pid);
    insert_optional(&mut job, "stdoutTail", stdout_tail);
    insert_optional(&mut job, "stderrTail", stderr_tail);
    insert_optional(&mut job, "outputTail", output_tail);
    job.insert("raw".to_owned(), item.clone());
    Some(Value::Object(job))
}
fn resolve_task_kind(record: &Value) -> Option<&'static str> {
    read_explicit_task_kind(record)
        .or_else(|| read_tool_task_kind(record))
        .or_else(|| read_background_type_task_kind(record))
}
fn read_explicit_task_kind(record: &Value) -> Option<&'static str> {
    let token = normalize_token(
        string_field(record, &["taskKind", "task_kind", "taskType", "task_type"]).as_deref(),
    );
    task_kind_from_token(&token)
}
fn read_tool_task_kind(record: &Value) -> Option<&'static str> {
    let token = normalize_token(string_field(record, &["toolName", "tool_name"]).as_deref());
    if token == "task" {
        return Some("agent");
    }
    task_kind_from_token(&token)
}
fn read_background_type_task_kind(record: &Value) -> Option<&'static str> {
    let token = normalize_token(string_field(record, &["type", "kind", "category"]).as_deref());
    match token.as_str() {
        "local_agent" | "subagent" | "background_agent" | "agent_background" => Some("agent"),
        "local_bash" | "background_bash" | "background_shell" | "bash_background"
        | "shell_background" => Some("bash"),
        _ => None,
    }
}
fn task_kind_from_token(token: &str) -> Option<&'static str> {
    match token {
        "agent" | "local_agent" | "subagent" | "background_agent" => Some("agent"),
        "bash" | "local_bash" | "shell" | "terminal" | "background_bash" | "background_shell" => {
            Some("bash")
        }
        _ => None,
    }
}

/// TS `readDisplayCommand`：agent 优先 description/summary/title/label，bash 优先命令本身。
fn read_display_command(record: &Value, task_kind: &str) -> Option<String> {
    if task_kind == "agent" {
        return string_field(record, &["description", "summary", "title", "label"])
            .or_else(|| read_command(record));
    }
    read_command(record)
        .or_else(|| string_field(record, &["description", "summary", "title", "label"]))
}
fn read_command(record: &Value) -> Option<String> {
    let direct = command_from_value(field(record, "command"))
        .or_else(|| command_from_value(field(record, "cmd")))
        .or_else(|| command_from_value(field(record, "script")));
    if direct.is_some() {
        return direct;
    }
    let input = field(record, "input").filter(|value| value.is_object())?;
    command_from_value(field(input, "command"))
        .or_else(|| command_from_value(field(input, "cmd")))
        .or_else(|| command_from_value(field(input, "script")))
}
fn command_from_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().trim().to_owned())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        _ => None,
    }
}

/// TS `normalizeToken`：trim + 小写 + 连续非 `[a-z0-9]` 折成 `_`（正则带 unicode 标志）。
fn normalize_token(value: Option<&str>) -> String {
    let lower = value.unwrap_or_default().trim().to_lowercase();
    let mut token = String::with_capacity(lower.len());
    let mut previous_underscore = false;
    for character in lower.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            token.push(character);
            previous_underscore = false;
        } else if !previous_underscore && !token.is_empty() {
            token.push('_');
            previous_underscore = true;
        }
    }
    token.trim_end_matches('_').to_owned()
}
fn normalize_status(value: Option<&str>) -> &'static str {
    match normalize_token(value).as_str() {
        "queued" | "scheduled" | "starting" | "pending" => "pending",
        "running" | "in_progress" | "started" | "active" => "running",
        "completed" | "complete" | "success" | "succeeded" | "done" => "completed",
        "failed" | "failure" | "error" | "spawn_error" => "failed",
        "killed" | "cancelled" | "canceled" | "stopped" | "terminated" | "timed_out"
        | "timeout" => "killed",
        "lost" | "unknown" => "lost",
        _ => "running",
    }
}
fn read_job_id(record: &Value, command: &str) -> String {
    let explicit = string_field(
        record,
        &[
            // ESCode Protocol 后台任务取消入口按 taskId 查找 runtime task 记录。
            "taskId",
            "task_id",
            "backgroundTaskId",
            "background_task_id",
            "jobId",
            "job_id",
            "backgroundJobId",
            "background_job_id",
            "id",
        ],
    );
    if let Some(explicit) = explicit {
        return explicit;
    }
    let started_at = read_instant_ms(
        record,
        &["startedAt", "started_at", "startTime", "start_time"],
    );
    let pid = read_number_field(record, &["pid", "processId", "process_id"]);
    let pid = pid.map_or_else(|| "no-pid".to_owned(), js_integer_text);
    let started_at = started_at.map_or_else(|| "no-start".to_owned(), js_integer_text);
    format!("background-task:{pid}:{started_at}:{command}")
}

/// TS 数字进模板串的口径（整数不写小数点）。
fn js_integer_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// TS `stringValue`：非空字符串才算有值（不 trim）。
fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let record = value.as_object()?;
    for key in keys {
        let Some(candidate) = record.get(*key).and_then(Value::as_str) else {
            continue;
        };
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    None
}
fn read_number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    let record = value.as_object()?;
    for key in keys {
        let Some(candidate) = record.get(*key) else {
            continue;
        };
        if let Some(number) = candidate.as_f64().filter(|number| number.is_finite()) {
            return Some(number);
        }
        if let Some(text) = candidate
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            && let Ok(number) = text.parse::<f64>()
            && number.is_finite()
        {
            return Some(number);
        }
    }
    None
}
fn read_boolean_field(value: &Value, keys: &[&str]) -> Option<bool> {
    let record = value.as_object()?;
    for key in keys {
        let Some(candidate) = record.get(*key) else {
            continue;
        };
        match candidate {
            Value::Bool(flag) => return Some(*flag),
            Value::String(text) => match text.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" => return Some(true),
                "false" | "no" | "0" => return Some(false),
                _ => {}
            },
            _ => {}
        }
    }
    None
}

/// TS `readInstantMs`：数字（或数字串）优先；字符串再走 `Date.parse`。
fn read_instant_ms(value: &Value, keys: &[&str]) -> Option<f64> {
    if let Some(number) = read_number_field(value, keys) {
        return Some(number);
    }
    let record = value.as_object()?;
    for key in keys {
        let Some(text) = record
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        if let Some(parsed) = parse_iso_instant_ms(text) {
            return Some(parsed);
        }
    }
    None
}

/// ISO 8601 子集（`YYYY-MM-DDTHH:MM:SS[.sss][Z|±HH:MM]`）→ epoch ms（UTC）。
pub(super) fn parse_iso_instant_ms(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let digits = |slice: &str| slice.parse::<i64>().ok();
    let year = digits(&text[0..4])?;
    let month = digits(&text[5..7])?;
    let day = digits(&text[8..10])?;
    let hour = digits(&text[11..13])?;
    let minute = digits(&text[14..16])?;
    let second = digits(&text[17..19])?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis: i64 = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.chars().take_while(char::is_ascii_digit).count();
        let value = fraction[..digits].parse::<i64>().ok()?;
        millis = match digits {
            0 => 0,
            1 => value * 100,
            2 => value * 10,
            _ => value / 10_i64.pow((digits - 3) as u32),
        };
        rest = &fraction[digits..];
    }
    let offset_minutes = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, minutes) = rest[1..].split_once(':')?;
            let hours = hours.parse::<i64>().ok()?;
            let minutes = minutes.parse::<i64>().ok()?;
            sign * (hours * 60 + minutes)
        }
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some((seconds * 1000 + millis) as f64)
}

/// Howard Hinnant 的 civil → days 算法（与 `Date.UTC` 同口径）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// JS 数字写进 JSON 的口径：整数值不写小数点（`JSON.stringify(42)` 是 `42`）。
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}
fn insert_number(record: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        record.insert(key.to_owned(), number_value(value));
    }
}
fn insert_optional(record: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        record.insert(key.to_owned(), Value::from(value));
    }
}
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}
