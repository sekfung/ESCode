use std::path::Path;
use zcode_cli_core_api as contract;
use zcode_cli_domain as domain;
pub mod child_env;
mod context_git;
pub mod context_source;
pub mod credential_cipher;
pub mod credential_store;
pub mod file_lock;
pub mod image_budget;
pub mod legacy_paths;
pub mod tls_ca;
mod realpath;
pub use realpath::{realpath, realpath_for_create, realpath_sync, simplify_verbatim};
mod question_timing;
pub use context_source::WorkspaceContext;
pub use question_timing::question_timing;
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn workspace_identity(identity: Option<&str>, workspace_path: &Path) -> String {
    identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| workspace_path.to_string_lossy().into_owned())
}
/// 本地年月（WebSearch 描述中的当前月份，TS 每次读取描述时按本地时间重新生成）。
pub fn local_year_month() -> (i32, u32) {
    use chrono::Datelike;
    let now = chrono::Local::now();
    (now.year(), now.month())
}
pub struct SystemClock;
impl contract::RuntimeClock for SystemClock {
    fn now(&self) -> u64 {
        now()
    }
    fn id(&self) -> String {
        id()
    }
    fn local_date(&self) -> Option<String> {
        Some(chrono::Local::now().format("%Y-%m-%d").to_string())
    }
    fn tz_offset_ms(&self, time_zone: &str, at_ms: i64) -> i64 {
        tz_offset_ms(time_zone, at_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_identity;
    use std::path::Path;

    #[test]
    fn identity_prefers_trimmed_remote_key_and_falls_back_to_path() {
        assert_eq!(
            workspace_identity(Some("  remote-1  "), Path::new("/repo")),
            "remote-1"
        );
        assert_eq!(workspace_identity(Some("  "), Path::new("/repo")), "/repo");
        assert_eq!(workspace_identity(None, Path::new("/repo")), "/repo");
    }
}

/// TS `resolveTzOffsetMs`（usage-stats-builder.ts）：`timeZone` 在 `at_ms` 时刻相对 UTC 的偏移（ms）。
/// Rust 不带 IANA 时区库：UTC 与本机时区（App 传的就是本机时区）精确，其余按 TS 无法解析时的回退取 0。
pub fn tz_offset_ms(time_zone: &str, at_ms: i64) -> i64 {
    use chrono::{Offset, TimeZone};
    if matches!(time_zone, "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT") {
        return 0;
    }
    if iana_time_zone::get_timezone().ok().as_deref() != Some(time_zone) {
        return 0;
    }
    chrono::Local
        .timestamp_millis_opt(at_ms)
        .single()
        .map_or(0, |at| i64::from(at.offset().fix().local_minus_utc()) * 1000)
}
