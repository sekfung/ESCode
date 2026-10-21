//! automation 写面与调度状态机（TS `automationRepo.ts` 的 Rust 子集）。
//!
//! 仓库只做存储与原子状态迁移；cron 表达式解析 / next_run_at 计算由 service 算好后传入。
//!
//! 行映射与各类迁移按关注点拆分：`model`（表结构/读写）、`create`（新建与立即运行）、
//! `lifecycle`（编辑/重试/跳过）、`dispatch`（认领与派发结算）。

mod create;
mod dispatch;
mod lifecycle;
mod model;

pub use model::now_ms;
