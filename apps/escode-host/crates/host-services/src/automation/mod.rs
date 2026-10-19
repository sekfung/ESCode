//! automation 本地管理面（docs/specs/rust-u2-automation.md）。
//!
//! 当前实现 list/listAll/delete/setEnabled/listRuns/deleteRun；create/update/restart/runNow
//! 依赖 cron 计算与派发器，仍为显式缺口。

mod repo;
mod service;

pub use service::AutomationService;
