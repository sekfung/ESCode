//! Host 基础设施（docs/specs/rust-u2-host-skeleton.md §3）：有界通道与原子写。
//! TaskScope 直接复用 `escode-cli-core-api::scope`（阶段 1 库化红利），进程回收复用
//! `escode-cli-host` 的 process_group/process_tree，本 crate 不重复实现。
pub mod atomic_write;
pub mod bounded;
