//! automation 本地管理面（docs/specs/rust-u2-automation.md）。
//!
//! 管理面：list/listAll/delete/setEnabled/listRuns/deleteRun + create/update/restart/runNow；
//! 调度状态机（claimDue / manual claim / dispatched / failed）在 repo，Host 负责派发。

pub mod cron;
mod repo;
mod repo_write;
mod service;

pub use service::AutomationService;
