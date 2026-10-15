//! Host 服务域（rust-u2-host-skeleton.md §5 优先序）：按域拆分，逐域交付。
//! 当前域：settings（唯一写者，与 TS Host 同文件同格式）、tasks-index（只读投影，见
//! docs/specs/rust-u2-tasks-index.md）、task_projection（agent 协议快照 → 任务快照/meta
//! 投影，见 docs/specs/rust-u2-tasks-index.md §6.4）、session_params（create/resume 参数整形，
//! 见同 spec §5）。
pub mod session_params;
pub mod settings;
pub mod task_projection;
pub mod tasks_index;
