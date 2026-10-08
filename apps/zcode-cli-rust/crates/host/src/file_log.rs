//! Rust runtime 的 JSONL 文件日志与 7 天保留（docs/specs/rust-file-log.md），对齐 Node `adapters/src/logging`。
//! 修复：Rust 原先只写 stderr，桌面「导出日志 / 反馈」打包的 `~/.zcode/cli/log` 里没有 runtime 日志。
//!
//! 所有者：进程级唯一写者线程，调用方只投递条目、不等待；写入失败不影响运行。
use chrono::{Duration, Local, NaiveDate, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{OnceLock, mpsc},
};

const RETENTION_DAYS: i64 = 7;
const CLEANUP_DELAY: std::time::Duration = std::time::Duration::from_secs(60);
const FILE_PREFIX: &str = "zcode-rust-";
const REDACT_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

struct Writer {
    dir: PathBuf,
    min: Level,
    lines: mpsc::Sender<String>,
}

static WRITER: OnceLock<Option<Writer>> = OnceLock::new();

/// 目录：`ZCODE_LOG_DIR` ?? `~/.zcode/cli/log`（Node getDefaultLogDir）。
fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ZCODE_LOG_DIR").filter(|v| !v.is_empty()) {
        return Some(dir.into());
    }
    crate::legacy_paths::home()
        .ok()
        .map(|home| home.join(".zcode").join("cli").join("log"))
}

fn writer() -> Option<&'static Writer> {
    WRITER
        .get_or_init(|| {
            let dir = log_dir()?;
            let min = if std::env::var("ZCODE_RUNTIME_ENV").as_deref() == Ok("development") {
                Level::Debug
            } else {
                Level::Info
            };
            let (lines, rx) = mpsc::channel::<String>();
            let target = dir.clone();
            std::thread::Builder::new()
                .name("zcode-file-log".into())
                .spawn(move || {
                    for line in rx {
                        let _ = append(&target, &line);
                    }
                })
                .ok()?;
            Some(Writer { dir, min, lines })
        })
        .as_ref()
}

fn append(dir: &Path, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(Local::now().date_naive()));
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

fn file_name(date: NaiveDate) -> String {
    format!("{FILE_PREFIX}{}.jsonl", date.format("%Y-%m-%d"))
}

/// Node `toSerializableEntry` 同形的一行；`None` 字段省略。
fn entry(
    level: Level,
    event: &str,
    module: &str,
    message: &str,
    session_id: Option<&str>,
    context: Value,
) -> String {
    let mut value = Map::new();
    value.insert("timestamp".into(), Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true).into());
    value.insert("level".into(), level.name().into());
    value.insert("event".into(), event.into());
    value.insert("module".into(), module.into());
    value.insert("message".into(), message.into());
    if let Some(session) = session_id {
        value.insert("sessionId".into(), session.into());
    }
    if !context.is_null() {
        value.insert("context".into(), redact(context, 0));
    }
    Value::Object(value).to_string()
}

/// Node `DefaultLogRedactor`：敏感键名的值替换，深度超过 8 截断。
fn redact(value: Value, depth: usize) -> Value {
    if depth > REDACT_DEPTH {
        return "[Redacted:DepthLimit]".into();
    }
    match value {
        Value::Array(items) => items.into_iter().map(|v| redact(v, depth + 1)).collect(),
        Value::Object(map) => map
            .into_iter()
            .map(|(key, v)| {
                let v = if sensitive(&key) { "[Redacted]".into() } else { redact(v, depth + 1) };
                (key, v)
            })
            .collect::<Map<_, _>>()
            .into(),
        other => other,
    }
}

fn sensitive(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["apikey", "api-key", "api_key", "authorization", "cookie", "credential", "password", "secret", "token"]
        .iter()
        .any(|needle| key.contains(needle))
}

/// 记一条日志。warn / error 同时写 stderr，保留原有诊断输出（Host 从 stderr 收集）。
pub fn log(level: Level, event: &str, module: &str, message: &str, session_id: Option<&str>, context: Value) {
    if level >= Level::Warn {
        let _ = writeln!(std::io::stderr().lock(), "zcode-cli-rust: {message}");
    }
    let Some(writer) = writer() else { return };
    if level < writer.min {
        return;
    }
    let _ = writer.lines.send(entry(level, event, module, message, session_id, context));
}

pub fn info(event: &str, module: &str, message: &str, context: Value) {
    log(Level::Info, event, module, message, None, context);
}

pub fn warn(event: &str, module: &str, message: &str, context: Value) {
    log(Level::Warn, event, module, message, None, context);
}

pub fn error(event: &str, module: &str, message: &str, context: Value) {
    log(Level::Error, event, module, message, None, context);
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Cleanup {
    scanned: usize,
    deleted: Vec<String>,
    failed: Vec<String>,
}

/// 本 runtime 日文件的日期；名字必须是 `zcode-rust-YYYY-MM-DD.jsonl` 且为合法日期（Node parseLogFileDate）。
fn file_date(name: &str) -> Option<NaiveDate> {
    let date = name.strip_prefix(FILE_PREFIX)?.strip_suffix(".jsonl")?;
    let bytes = date.as_bytes();
    let shape = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    shape.then(|| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())?
}

/// 删除早于 `today - 7 + 1` 的本 runtime 日文件；单个失败不影响其余。目录不存在视为完成。
fn cleanup(dir: &Path, today: NaiveDate, remove: &dyn Fn(&Path) -> std::io::Result<()>) -> std::io::Result<Cleanup> {
    let cutoff = today - Duration::days(RETENTION_DAYS - 1);
    let mut result = Cleanup::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(error) => return Err(error),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let Some(date) = file_date(&name) else { continue };
        result.scanned += 1;
        if date >= cutoff {
            continue;
        }
        match remove(&dir.join(&name)) {
            Ok(()) => result.deleted.push(name),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => result.failed.push(name),
        }
    }
    Ok(result)
}

/// 启动 60 秒后在后台清理一次（Node LOG_CLEANUP_STARTUP_DELAY_MS），不阻塞请求与退出。
pub fn schedule_retention() {
    let Some(dir) = writer().map(|w| w.dir.clone()) else { return };
    info("log.retention.cleanup.scheduled", "host.file_log", "Log retention cleanup scheduled", json!({"retentionDays": RETENTION_DAYS}));
    let _ = std::thread::Builder::new().name("zcode-log-retention".into()).spawn(move || {
        std::thread::sleep(CLEANUP_DELAY);
        match cleanup(&dir, Local::now().date_naive(), &|path| std::fs::remove_file(path)) {
            Ok(result) => {
                for name in &result.failed {
                    warn("log.retention.delete.failed", "host.file_log", "Log retention delete failed", json!({"fileName": name}));
                }
                log(Level::Debug, "log.retention.cleanup.completed", "host.file_log", "Log retention cleanup completed", None,
                    json!({"scannedFiles": result.scanned, "deletedFiles": result.deleted.len(), "failedFiles": result.failed.len()}));
            }
            Err(error) => warn("log.retention.cleanup.failed", "host.file_log", "Log retention cleanup failed", json!({"errorKind": format!("{:?}", error.kind())})),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_matches_node_shape_and_redacts() {
        let line = entry(
            Level::Info,
            "runtime.started",
            "cli",
            "started",
            Some("sess_1"),
            json!({"apiKey": "k", "nested": {"Authorization": "b", "ok": 1}, "list": [{"refresh_token": "t"}]}),
        );
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["level"], "info");
        assert_eq!(value["event"], "runtime.started");
        assert_eq!(value["sessionId"], "sess_1");
        assert_eq!(value["context"]["apiKey"], "[Redacted]");
        assert_eq!(value["context"]["nested"]["Authorization"], "[Redacted]");
        assert_eq!(value["context"]["nested"]["ok"], 1);
        assert_eq!(value["context"]["list"][0]["refresh_token"], "[Redacted]");
        assert!(value["timestamp"].as_str().unwrap().ends_with('Z'));
        let bare: Value = serde_json::from_str(&entry(Level::Warn, "e", "m", "x", None, Value::Null)).unwrap();
        assert!(bare.get("sessionId").is_none() && bare.get("context").is_none());
    }

    #[test]
    fn redaction_stops_at_depth_limit() {
        let mut value = json!("leaf");
        for _ in 0..12 {
            value = json!({ "a": value });
        }
        let text = redact(value, 0).to_string();
        assert!(text.contains("[Redacted:DepthLimit]"));
    }

    #[test]
    fn file_dates_are_strict() {
        assert!(file_date("zcode-rust-2026-10-08.jsonl").is_some());
        assert!(file_date("zcode-rust-2026-02-30.jsonl").is_none());
        assert!(file_date("zcode-2026-10-08.jsonl").is_none());
        assert!(file_date("zcode-rust-2026-1-08.jsonl").is_none());
    }

    #[test]
    fn cleanup_keeps_seven_days_and_tolerates_failures() {
        let dir = std::env::temp_dir().join(format!("zcode-file-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "zcode-rust-2026-10-01.jsonl",
            "zcode-rust-2026-10-02.jsonl",
            "zcode-rust-2026-09-30.jsonl",
            "zcode-rust-2026-09-29.jsonl",
            "zcode-2026-09-01.jsonl",
            "zcode-rust-2026-02-30.jsonl",
        ] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let today = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        // 截止日期 = 10-08 − 7 + 1 = 10-02：10-01 及更早删除；29 号删除失败不影响 30 号。
        let result = cleanup(&dir, today, &|path| {
            if path.ends_with("zcode-rust-2026-09-29.jsonl") {
                Err(std::io::Error::other("busy"))
            } else {
                std::fs::remove_file(path)
            }
        })
        .unwrap();
        assert_eq!(result.scanned, 4);
        assert_eq!(result.deleted, vec!["zcode-rust-2026-09-30.jsonl", "zcode-rust-2026-10-01.jsonl"]);
        assert_eq!(result.failed, vec!["zcode-rust-2026-09-29.jsonl"]);
        assert!(dir.join("zcode-rust-2026-10-02.jsonl").exists());
        assert!(dir.join("zcode-2026-09-01.jsonl").exists());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(cleanup(&dir, today, &|p| std::fs::remove_file(p)).unwrap(), Cleanup::default());
    }
}
