//! 四类外部 agent 的来源路径表（TS `SUPPORTED_*_AGENT_SOURCES`）。

use super::agents::Agent;
use super::sources::{McpFormat, McpSource, PathSource};

const CLAUDE_SETTINGS: &[&str] = &[".claude", "settings.json"];
const MCP_JSON: &[&str] = &[".mcp.json"];

pub const SKILL_SOURCES: &[PathSource] = &[
    PathSource {
        agent: Agent::ClaudeCode,
        project: &[".claude", "skills"],
        global: &[".claude", "skills"],
    },
    PathSource {
        agent: Agent::CodexCli,
        project: &[".codex", "skills"],
        global: &[".codex", "skills"],
    },
    PathSource {
        agent: Agent::OpenCode,
        project: &[".opencode", "skills"],
        global: &[".config", "opencode", "skills"],
    },
    PathSource {
        agent: Agent::OpenClaw,
        project: &["skills"],
        global: &[".openclaw", "skills"],
    },
    PathSource {
        agent: Agent::Augment,
        project: &[".augment", "skills"],
        global: &[".augment", "skills"],
    },
    PathSource {
        agent: Agent::Continue,
        project: &[".continue", "skills"],
        global: &[".continue", "skills"],
    },
    PathSource {
        agent: Agent::Goose,
        project: &[".goose", "skills"],
        global: &[".config", "goose", "skills"],
    },
    PathSource {
        agent: Agent::QwenCode,
        project: &[".qwen", "skills"],
        global: &[".qwen", "skills"],
    },
    PathSource {
        agent: Agent::Qode,
        project: &[".qoder", "skills"],
        global: &[".qoder", "skills"],
    },
    PathSource {
        agent: Agent::QodeCn,
        project: &[".qoder", "skills"],
        global: &[".qoder-cn", "skills"],
    },
    PathSource {
        agent: Agent::Windsurf,
        project: &[".windsurf", "skills"],
        global: &[".codeium", "windsurf", "skills"],
    },
    PathSource {
        agent: Agent::Trae,
        project: &[".trae", "skills"],
        global: &[".trae", "skills"],
    },
    PathSource {
        agent: Agent::TraeCn,
        project: &[".trae", "skills"],
        global: &[".trae-cn", "skills"],
    },
    PathSource {
        agent: Agent::KiroCli,
        project: &[".kiro", "skills"],
        global: &[".kiro", "skills"],
    },
    PathSource {
        agent: Agent::Roo,
        project: &[".roo", "skills"],
        global: &[".roo", "skills"],
    },
    PathSource {
        agent: Agent::CodeBuddy,
        project: &[".codebuddy", "skills"],
        global: &[".codebuddy", "skills"],
    },
];

pub const COMMAND_SOURCES: &[PathSource] = &[
    PathSource {
        agent: Agent::ClaudeCode,
        project: &[".claude", "commands"],
        global: &[".claude", "commands"],
    },
    PathSource {
        agent: Agent::CodexCli,
        project: &[".codex", "commands"],
        global: &[".codex", "commands"],
    },
    PathSource {
        agent: Agent::OpenCode,
        project: &[".opencode", "commands"],
        global: &[".config", "opencode", "commands"],
    },
    PathSource {
        agent: Agent::OpenClaw,
        project: &["commands"],
        global: &[".openclaw", "commands"],
    },
    PathSource {
        agent: Agent::Augment,
        project: &[".augment", "commands"],
        global: &[".augment", "commands"],
    },
    PathSource {
        agent: Agent::Continue,
        project: &[".continue", "commands"],
        global: &[".continue", "commands"],
    },
    PathSource {
        agent: Agent::Goose,
        project: &[".goose", "commands"],
        global: &[".config", "goose", "commands"],
    },
    PathSource {
        agent: Agent::QwenCode,
        project: &[".qwen", "commands"],
        global: &[".qwen", "commands"],
    },
    PathSource {
        agent: Agent::Qode,
        project: &[".qoder", "commands"],
        global: &[".qoder", "commands"],
    },
    PathSource {
        agent: Agent::QodeCn,
        project: &[".qoder", "commands"],
        global: &[".qoder-cn", "commands"],
    },
    PathSource {
        agent: Agent::Windsurf,
        project: &[".windsurf", "commands"],
        global: &[".codeium", "windsurf", "commands"],
    },
    PathSource {
        agent: Agent::Trae,
        project: &[".trae", "commands"],
        global: &[".trae", "commands"],
    },
    PathSource {
        agent: Agent::KiroCli,
        project: &[".kiro", "commands"],
        global: &[".kiro", "commands"],
    },
    PathSource {
        agent: Agent::Roo,
        project: &[".roo", "commands"],
        global: &[".roo", "commands"],
    },
    PathSource {
        agent: Agent::CodeBuddy,
        project: &[".codebuddy", "commands"],
        global: &[".codebuddy", "commands"],
    },
];

pub const PLUGIN_SOURCES: &[PathSource] = &[
    PathSource {
        agent: Agent::ClaudeCode,
        project: &[".claude", "plugins"],
        global: &[".claude", "plugins"],
    },
    PathSource {
        agent: Agent::CodexCli,
        project: &[".codex", "plugins"],
        global: &[".codex", "plugins"],
    },
    PathSource {
        agent: Agent::OpenCode,
        project: &[".opencode", "plugins"],
        global: &[".config", "opencode", "plugins"],
    },
    PathSource {
        agent: Agent::OpenClaw,
        project: &["plugins"],
        global: &[".openclaw", "plugins"],
    },
    PathSource {
        agent: Agent::Augment,
        project: &[".augment", "plugins"],
        global: &[".augment", "plugins"],
    },
    PathSource {
        agent: Agent::Continue,
        project: &[".continue", "plugins"],
        global: &[".continue", "plugins"],
    },
    PathSource {
        agent: Agent::Goose,
        project: &[".goose", "plugins"],
        global: &[".config", "goose", "plugins"],
    },
    PathSource {
        agent: Agent::QwenCode,
        project: &[".qwen", "plugins"],
        global: &[".qwen", "plugins"],
    },
    PathSource {
        agent: Agent::Qode,
        project: &[".qoder", "plugins"],
        global: &[".qoder", "plugins"],
    },
    PathSource {
        agent: Agent::QodeCn,
        project: &[".qoder", "plugins"],
        global: &[".qoder-cn", "plugins"],
    },
    PathSource {
        agent: Agent::Windsurf,
        project: &[".windsurf", "plugins"],
        global: &[".codeium", "windsurf", "plugins"],
    },
    PathSource {
        agent: Agent::Trae,
        project: &[".trae", "plugins"],
        global: &[".trae", "plugins"],
    },
    PathSource {
        agent: Agent::KiroCli,
        project: &[".kiro", "plugins"],
        global: &[".kiro", "plugins"],
    },
    PathSource {
        agent: Agent::Roo,
        project: &[".roo", "plugins"],
        global: &[".roo", "plugins"],
    },
    PathSource {
        agent: Agent::CodeBuddy,
        project: &[".codebuddy", "plugins"],
        global: &[".codebuddy", "plugins"],
    },
];

pub const MCP_SOURCES: &[McpSource] = &[
    McpSource {
        agent: Agent::ClaudeCode,
        project_files: &[CLAUDE_SETTINGS, MCP_JSON],
        global_files: &[&[".claude", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::CodexCli,
        project_files: &[&[".codex", "config.toml"]],
        global_files: &[&[".codex", "config.toml"]],
        format: McpFormat::CodexToml,
    },
    McpSource {
        agent: Agent::OpenCode,
        project_files: &[&[".opencode", "opencode.json"]],
        global_files: &[&[".config", "opencode", "opencode.json"]],
        format: McpFormat::McpJson,
    },
    McpSource {
        agent: Agent::OpenClaw,
        project_files: &[&["settings.json"]],
        global_files: &[&[".openclaw", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::QwenCode,
        project_files: &[&[".qwen", "settings.json"]],
        global_files: &[&[".qwen", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::Qode,
        project_files: &[&[".qoder", "settings.json"]],
        global_files: &[&[".qoder", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::QodeCn,
        project_files: &[&[".qoder", "settings.json"]],
        global_files: &[&[".qoder-cn", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::Trae,
        project_files: &[&[".trae", "settings.json"]],
        global_files: &[&[".trae", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::KiroCli,
        project_files: &[&[".kiro", "settings.json"]],
        global_files: &[&[".kiro", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::Roo,
        project_files: &[&[".roo", "settings.json"]],
        global_files: &[&[".roo", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::CodeBuddy,
        project_files: &[&[".codebuddy", "settings.json"]],
        global_files: &[&[".codebuddy", "settings.json"]],
        format: McpFormat::McpServersJson,
    },
    McpSource {
        agent: Agent::Agents,
        project_files: &[&[".agents", "mcp.json"]],
        global_files: &[&[".agents", "mcp.json"]],
        format: McpFormat::McpServersJson,
    },
];
