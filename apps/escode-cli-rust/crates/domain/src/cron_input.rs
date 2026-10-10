//! Cron 工具入参校验，逐条对齐 TS `contracts/src/tools/automation.ts` 的 zod schema（strict、trim、refine）。
//! 见 docs/specs/rust-cron.md。

use serde_json::{Map, Value};

const UNITS: [&str; 6] = ["minute", "hourly", "daily", "weekly", "monthly", "yearly"];

/// 按 TS schema 校验并返回解析结果（字符串 trim）。错误文案为 Rust 自拟（TS 为 ZodError）。
pub fn parse(tool: &str, input: &Value) -> Result<Map<String, Value>, String> {
    let record = input.as_object().ok_or("Tool input must be an object")?;
    let allowed: &[&str] = match tool {
        "CronCreate" => &[
            "cron",
            "delayMinutes",
            "prompt",
            "title",
            "recurring",
            "maxRuns",
            "intervalUnit",
            "interval",
        ],
        "CronUpdate" => &[
            "id",
            "cron",
            "prompt",
            "title",
            "recurring",
            "maxRuns",
            "intervalUnit",
            "interval",
        ],
        "CronDelete" => &["id"],
        "CronList" => &[],
        _ => return Err(format!("Unknown tool {tool}")),
    };
    if let Some(key) = record.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("Unrecognized key: {key}"));
    }
    let mut out = Map::new();
    let text = |key: &str, required: bool, out: &mut Map<String, Value>| -> Result<(), String> {
        match record.get(key) {
            None if !required => Ok(()),
            None => Err(format!("{key} is required")),
            Some(Value::String(value)) => {
                let trimmed = crate::web_fetch::js_trim(value);
                if trimmed.is_empty() {
                    return Err(format!("{key} must not be empty"));
                }
                out.insert(key.into(), trimmed.into());
                Ok(())
            }
            Some(_) => Err(format!("{key} must be a string")),
        }
    };
    let int = |value: &Value| value.as_f64().filter(|n| n.fract() == 0.0 && n.is_finite());
    match tool {
        "CronList" => return Ok(out),
        "CronDelete" => {
            text("id", true, &mut out)?;
            return Ok(out);
        }
        "CronUpdate" => text("id", true, &mut out)?,
        _ => {}
    }
    text("cron", false, &mut out)?;
    if tool == "CronCreate" {
        match record.get("delayMinutes") {
            None => {}
            Some(Value::Null) => {
                out.insert("delayMinutes".into(), Value::Null);
            }
            Some(value) => {
                let n = int(value).ok_or("delayMinutes must be an integer")?;
                if !(n > 0.0 && n <= 525_600.0) {
                    return Err("delayMinutes must be between 1 and 525600".into());
                }
                out.insert("delayMinutes".into(), value.clone());
            }
        }
    }
    text("prompt", tool == "CronCreate", &mut out)?;
    text("title", true, &mut out)?;
    if let Some(value) = record.get("recurring") {
        out.insert(
            "recurring".into(),
            value.as_bool().ok_or("recurring must be a boolean")?.into(),
        );
    }
    match record.get("maxRuns") {
        None => {}
        Some(Value::Null) if tool == "CronUpdate" => {
            out.insert("maxRuns".into(), Value::Null);
        }
        Some(value) => {
            let n = int(value).ok_or("maxRuns must be an integer")?;
            if n <= 0.0 {
                return Err("maxRuns must be positive".into());
            }
            out.insert("maxRuns".into(), value.clone());
        }
    }
    if let Some(value) = record.get("intervalUnit") {
        let unit = value
            .as_str()
            .filter(|u| UNITS.contains(u))
            .ok_or("invalid intervalUnit")?;
        out.insert("intervalUnit".into(), unit.into());
    }
    if let Some(value) = record.get("interval") {
        let n = int(value).ok_or("interval must be an integer")?;
        if !(1.0..=200.0).contains(&n) {
            return Err("interval must be between 1 and 200".into());
        }
        out.insert("interval".into(), value.clone());
    }
    refine(tool, &out)?;
    Ok(out)
}

/// TS 各条 `.refine` 按声明顺序全部执行（zod 的 refine 失败不中断后续 refine），失败时 handler 抛出的
/// ZodError 文案（`JSON.stringify(issues, null, 2)`）即模型可见的工具错误。之前 Rust 只报第一条、文案为自拟缩写，
/// 与 Node 不同（docs/specs/rust-cron.md「refine 文案」；TS oracle 见 cron_corpus.json 的 `message`）。
fn refine(tool: &str, p: &Map<String, Value>) -> Result<(), String> {
    let has = |key: &str| p.contains_key(key);
    let relative = p.get("delayMinutes").is_some_and(Value::is_number);
    let carrier = has("intervalUnit");
    let paired = has("intervalUnit") == has("interval");
    let recurring_true = p.get("recurring") == Some(&Value::Bool(true));
    let recurring_false = p.get("recurring") == Some(&Value::Bool(false));
    let rules: Vec<(bool, &str, &str)> = if tool == "CronCreate" {
        vec![
            (!relative && !has("cron"), "cron is required when delayMinutes is not set", "cron"),
            (
                relative && has("cron"),
                "a relative delay must omit cron; the host computes the schedule from its real clock",
                "cron",
            ),
            (
                relative && recurring_true,
                "a relative delay creates a one-shot automation and cannot use recurring=true",
                "recurring",
            ),
            (relative && has("maxRuns"), "a relative delay runs once and cannot use maxRuns", "maxRuns"),
            (
                carrier && relative,
                "intervalUnit is a recurring carrier and cannot combine with a relative delayMinutes",
                "intervalUnit",
            ),
            (!paired, "intervalUnit and interval must be set together", "interval"),
            (
                carrier && recurring_false,
                "intervalUnit is a recurring carrier and requires recurring=true",
                "recurring",
            ),
            (
                carrier && has("maxRuns"),
                "intervalUnit is a recurring carrier and cannot combine with maxRuns",
                "maxRuns",
            ),
        ]
    } else {
        let fields = ["cron", "prompt", "title", "recurring", "maxRuns", "intervalUnit", "interval"];
        let clearing = p.get("maxRuns") == Some(&Value::Null);
        vec![
            (!fields.iter().any(|f| has(f)), "CronUpdate requires at least one field to update", ""),
            (
                clearing && !recurring_true,
                "Clearing maxRuns requires recurring=true in the same update",
                "maxRuns",
            ),
            (
                recurring_true && p.get("maxRuns").is_some_and(Value::is_number),
                "recurring=true cannot be combined with a numeric maxRuns",
                "maxRuns",
            ),
            (!paired, "intervalUnit and interval must be set together", "interval"),
            (
                carrier && recurring_false,
                "intervalUnit is a recurring carrier and cannot combine with recurring=false",
                "recurring",
            ),
            (
                carrier && has("maxRuns") && !(clearing && recurring_true),
                "intervalUnit is a recurring carrier and only allows maxRuns=null with recurring=true",
                "maxRuns",
            ),
        ]
    };
    let issues: Vec<Value> = rules
        .into_iter()
        .filter(|(failed, ..)| *failed)
        .map(|(_, message, path)| {
            let path: Vec<&str> = if path.is_empty() { vec![] } else { vec![path] };
            serde_json::json!({"code": "custom", "message": message, "path": path})
        })
        .collect();
    if issues.is_empty() {
        return Ok(());
    }
    Err(serde_json::to_string_pretty(&issues).unwrap_or_default())
}
