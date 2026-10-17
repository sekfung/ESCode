//! `mcp-sync` 域（docs/specs/rust-u2-sync.md §2）。
//!
//! 用户级/工作区级 MCP 配置的读取、`enable` 迁移、候选/导出/导入与写权限探测；
//! 运行态 `listWorkspaceMcpServerStatuses` 由 RPC 层转发运行时的 `mcp/list`。

mod config;
mod import;
mod paths;
mod read;
mod rewrite;
mod service;

pub use service::McpSyncService;
