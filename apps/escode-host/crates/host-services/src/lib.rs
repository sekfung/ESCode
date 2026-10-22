//! Host 服务域（rust-u2-host-skeleton.md §5 优先序）：按域拆分，逐域交付。
//! 当前域：settings（唯一写者，与 TS Host 同文件同格式）、tasks-index（只读投影，见
//! docs/specs/rust-u2-tasks-index.md）、task_projection（agent 协议快照 → 任务快照/meta
//! 投影，见 docs/specs/rust-u2-tasks-index.md §6.4）、session_params（create/resume 参数整形，
//! 见同 spec §5）。
pub mod api_client;
pub mod automation;
pub mod broadcast;
pub mod claude_import;
pub mod client_config;
pub mod client_scenes;
pub mod client_services;
pub mod commands;
pub mod credential;
pub mod data_dir;
pub mod file_index;
pub mod file_watcher;
pub mod gen_ui;
pub mod git;
pub mod hooks;
pub mod js_compat;
pub mod mcp_sync;
pub mod model_trajectory;
pub mod official_plugins;
pub mod onboarding;
pub(crate) mod ordered_json;
pub mod plugin_sync;
pub mod provider;
pub mod remote_sync;
pub mod serial;
pub mod session_params;
pub mod settings;
pub mod settings_sync;
pub mod skill_sync;
pub mod skills;
pub mod subagents;
pub mod svn;
pub mod system_probe;
pub mod task_projection;
pub mod tasks_index;
pub mod terminal;
