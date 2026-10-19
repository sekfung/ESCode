//! `off-peak-task` 管理面（docs/specs/rust-u2-off-peak.md）。
//!
//! 只实现 Host 自足的本地 SQLite 管理语义；取号/额度/创建依赖 Off-Peak 服务端与账号凭证，
//! 仍为显式缺口（见 spec §5）。

mod repo;
mod service;

#[cfg(test)]
mod tests;

pub use service::OffPeakService;
