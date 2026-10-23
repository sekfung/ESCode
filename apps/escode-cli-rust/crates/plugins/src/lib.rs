//! 插件域 crate（rust-unified-host.md P1 #9）。
//!
//! 从 `escode-cli-tools` 抽离插件发现、清单/校验、安装/卸载、市场读写与内置 MCP 服务器，
//! 只依赖 core-api / domain / host / net / protocol，不再依赖 tools。tool 侧通过
//! [`tool_port_plugins`] 与公开的配置/发现/路径模块访问本域，避免反向依赖。

mod atomic_dir;
pub mod bundled_skills;
mod config_file;
pub mod extension_config;
pub mod extension_plugins;
pub mod lexical_path;
pub mod mcp_config;
mod mcp_config_order;
pub mod mcp_node_repl;
pub mod mcp_rea;
pub mod mcp_serial;
mod official_plugins;
mod official_plugins_cache;
mod official_plugins_lock;
mod official_plugins_marketplace;
mod plugin_config;
mod plugin_describe;
mod plugin_describe_scan;
mod plugin_failure;
mod plugin_frontmatter;
mod plugin_git;
mod plugin_install;
mod plugin_install_cache;
mod plugin_install_entry;
mod plugin_install_source;
mod plugin_list;
mod plugin_list_components;
mod plugin_listing;
mod plugin_market_params;
mod plugin_market_source;
mod plugin_market_write;
pub mod plugin_marketplace;
mod plugin_mcp_vars;
mod plugin_overview;
mod plugin_reference;
mod plugin_skill_files;
mod plugin_suggested;
mod plugin_ui_surfaces;
mod plugin_uninstall;
mod plugin_validate;
mod plugin_validate_mcp;
#[cfg(test)]
mod plugin_validate_mcp_tests;
mod plugin_validate_shape;
mod plugin_zip;
pub mod ports;
pub mod tool_port_plugins;
