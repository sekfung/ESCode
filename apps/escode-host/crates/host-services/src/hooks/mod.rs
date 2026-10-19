//! `hooks` 服务域（docs/specs/rust-u2-hooks.md；TS `packages/services/src/hooks/`）。
//!
//! - `loadHooks` / `saveHooks`：Host 本地文件语义（发现、摘要、原子写、trust store 只读）；
//! - `grantWorkspaceHookTrust`：由 `host-rpc` 转发运行时 Agent authority，Host 不直接写 trust store。

mod config;
mod digest;
mod legacy;
mod paths;
mod schema;
mod service;
mod trust;
mod write;

#[cfg(test)]
mod tests;

pub use service::HooksService;
