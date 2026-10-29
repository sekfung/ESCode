use escode_cli_core_api as contract;
use escode_cli_domain as domain;

// P1 #9：插件域已抽到 escode-cli-plugins；保持 tools 内部既有的 `super::<module>` 路径。
pub(crate) use escode_cli_plugins::{
    bundled_skills, extension_config, extension_plugins, lexical_path, mcp_config, mcp_node_repl,
    mcp_rea, mcp_serial, plugin_marketplace, tool_port_plugins,
};
mod agent_profiles;
pub mod bash_git_safety;
mod bash_image;
mod bash_read_state;
mod browser_broker;
mod browser_broker_listen;
mod checkpoint_blobs;
mod custom_command_shell;
mod custom_commands;
#[cfg(test)]
mod custom_commands_tests;
mod disk_space;
mod edit_apply;
mod edit_match;
mod edit_quotes;
mod embedded_search;
mod eval_workflow_snippet;
mod file_atomic;
mod file_changes;
mod file_checkpoints;
mod file_missing;
mod file_patch;
mod file_rewind;
mod file_state;
mod hooks;
mod host_broker;
mod mcp_client_events;
mod mcp_connection;
mod mcp_http;
mod mcp_http_capture;
mod mcp_hub;
mod mcp_image_artifacts;
mod mcp_oauth_client;
mod mcp_oauth_credentials;
mod mcp_oauth_flow;
mod mcp_oauth_http;
mod mcp_oauth_store;
mod mcp_official_client;
mod mcp_official_stdio;
mod mcp_official_transport;
mod mcp_raw_capture;
mod mcp_request_meta;
mod mcp_sse;
mod mcp_ui;
mod model_admission;
mod plan_tools;
#[cfg(unix)]
mod process_tree;
mod project_memory;
mod read_image;
mod read_pdf;
mod workflow_analyzer;
mod workflow_host;
mod workflow_tools;
// 第 2 期存储层：ListSavedWorkflows / SaveWorkflow 与 `workflows/*` 管理方法随开关（第 1 期）接入。
mod save_workflow;
#[cfg(test)]
mod saved_workflow_tool_tests;
pub mod saved_workflows;
mod saved_workflows_hub;
mod saved_workflows_model;
mod saved_workflows_move;
mod saved_workflows_run;
#[cfg(test)]
mod saved_workflows_tests;
mod shell_select;
mod shell_snapshot;
mod tool_args;
mod tool_capability;
mod tool_files;
mod tool_files_read;
mod tool_process;
mod tool_search;
mod tool_shell;
mod tool_shell_support;
mod tool_skills;
mod tool_surface;
pub mod tools;
mod tools_tool_port_mcp;
mod web_fetch;
mod web_fetch_http;
#[cfg(windows)]
mod win_job;
mod workspace_tools;
mod workspace_tools_misc;
use escode_cli_host::{id, now};
pub use tools::WorkspaceTools;

/// 配置文件的 `network` 段（user 视图与 user + project 合并视图），进程启动时交给
/// `host::net_config`（docs/specs/rust-net-proxy.md「配置文件 `network` 段」）。
pub async fn network_file_config(
    cwd: &std::path::Path,
) -> anyhow::Result<escode_cli_host::net_config::NetworkFileLayers> {
    use escode_cli_host::net_config::{NetworkFileConfig, NetworkFileLayers};
    let layers = extension_config::load_layers(cwd).await?;
    Ok(NetworkFileLayers {
        user: NetworkFileConfig::from_config(&layers.user),
        workspace: NetworkFileConfig::from_config(&layers.merged()),
    })
}
