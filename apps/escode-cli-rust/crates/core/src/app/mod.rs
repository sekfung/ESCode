mod auxiliary;
mod commands;
mod engine;
mod engine_requests;
mod event_projection;
mod event_projection_dispatch;
mod host_requests;
mod model_config;
mod permission_flow;
mod permission_rules;
mod plan_mode;
mod plan_tool;
mod queries;
mod session_context_tool;
mod shell_preferences;
mod subscriptions;
mod waiters;
pub use crate::contract::{Event, RunEvent};
pub use engine::Engine;
mod input_validation;

mod agent_loop;
mod agent_loop_step;
mod agent_loop_surface;
mod agent_support;
mod memory_extraction;
mod memory_run;
mod tool_dispatch;
mod tool_input_checks;

mod context;
mod context_projection;
mod create_session;
mod cron_tool;
mod dynamic_workflow;
mod maintenance;
mod model_catalog_tool;
mod model_execution;
mod queue_control;
mod wiki_context;
mod wiki_requests;
mod workflow_run_tool;

mod assistant_feedback;
mod attachment_upload;
mod attachments;
mod input_attachments;
mod legacy_session;
mod model_io_preferences;
mod plugin_catalog;
mod plugin_jobs;
mod saved_workflow_hub;
mod selection_side_session;
mod session_close;
mod session_read;
mod session_residency;
mod session_resume;
mod session_title;
mod workflow_run_commands;
mod workflow_run_replies;
mod workflow_settings;

mod busy_input;
mod input_admission;
mod run;

mod question_timers;
mod question_tool;
mod questions;

mod todos;
mod web_fetch_tool;
mod web_search_tool;

mod claude_import;
mod cua_events;
mod goal_commands;
mod goal_events;
mod goal_loop;
mod hook_rows;
mod mcp;
mod mcp_client_events;
mod mcp_sampling;
mod permission_hooks;
mod session_debug;
mod session_list;
mod shell_notice;
mod skills;
mod stream_recovery;
mod subagent_completion;
mod subagent_query;
mod subagent_tools;
mod subagents;
mod telemetry;
mod tool_hooks;
mod turn_hooks;
mod usage_facts;
mod workflow_actors;
mod workflow_blocked;
mod workflow_notices;
mod workspace_hooks;

mod history_commands;

mod file_rewind;

mod file_changes;

mod background_events;

/// 任务捕获 panic 后统一构造运行时错误（对标仓库同名语义）：run/engine 只能把该任务当失败处理，
/// 错误串进入 `Event::Finished.error` 与遥测（docs/specs/rust-unified-host.md P0#1）。
pub(super) fn panicked(
    task: &'static str,
    panicked: escode_cli_core_api::Panicked,
) -> anyhow::Error {
    anyhow::anyhow!("{task} panicked: {}", panicked.message)
}
