//! `skill-sync` 域（docs/specs/rust-u2-sync.md §1）。
//!
//! Host 级服务：候选扫描、远端状态、gzip+tar 归档导出/导入与写权限探测。
//! 扫描策略复用 `skills` 域；归档格式与 TS 的 `serialize`/`deserialize` 协议互操作。

mod archive;
mod archive_tar;
mod candidates;
mod error;
mod import;
mod paths;
mod service;

pub use error::SkillSyncError;
pub use service::{DEFAULT_MAX_ARCHIVE_BYTES, ExportResult, SkillSyncService};
