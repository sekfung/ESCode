//! 外部 agent 的 source 路径表与路径解析（TS `SUPPORTED_*_AGENT_SOURCES` / `getExternal*Roots`）。

use std::path::{Path, PathBuf};

use super::agents::Agent;

pub use super::sources_tables::{COMMAND_SOURCES, MCP_SOURCES, PLUGIN_SOURCES, SKILL_SOURCES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceScope {
    User,
    Workspace,
}

impl SourceScope {
    /// `SettingsSyncSourceScope`：user → `global`，workspace → `project`。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "global",
            Self::Workspace => "project",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Providers,
    Skills,
    Commands,
    Plugins,
    McpServers,
}

impl Category {
    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "providers" => Self::Providers,
            "skills" => Self::Skills,
            "commands" => Self::Commands,
            "plugins" => Self::Plugins,
            "mcpServers" => Self::McpServers,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub struct PathSource {
    pub agent: Agent,
    pub project: &'static [&'static str],
    pub global: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub enum McpFormat {
    McpServersJson,
    McpJson,
    CodexToml,
}

#[derive(Clone, Copy)]
pub struct McpSource {
    pub agent: Agent,
    pub project_files: &'static [&'static [&'static str]],
    pub global_files: &'static [&'static [&'static str]],
    pub format: McpFormat,
}

pub struct SourceRoot {
    pub agent: Agent,
    pub path: PathBuf,
    pub scope: SourceScope,
    pub format: McpFormat,
}

fn path_roots(home: &Path, workspace: Option<&Path>, sources: &[PathSource]) -> Vec<SourceRoot> {
    let mut roots: Vec<SourceRoot> = sources
        .iter()
        .map(|source| SourceRoot {
            agent: source.agent,
            path: home.join(segments(source.global)),
            scope: SourceScope::User,
            format: McpFormat::McpServersJson,
        })
        .collect();
    if let Some(workspace) = workspace {
        roots.extend(sources.iter().map(|source| SourceRoot {
            agent: source.agent,
            path: workspace.join(segments(source.project)),
            scope: SourceScope::Workspace,
            format: McpFormat::McpServersJson,
        }));
    }
    roots
}

pub fn skill_roots(home: &Path, workspace: Option<&Path>) -> Vec<SourceRoot> {
    path_roots(home, workspace, SKILL_SOURCES)
}

pub fn command_roots(home: &Path, workspace: Option<&Path>) -> Vec<SourceRoot> {
    path_roots(home, workspace, COMMAND_SOURCES)
}

pub fn plugin_roots(home: &Path, workspace: Option<&Path>) -> Vec<SourceRoot> {
    path_roots(home, workspace, PLUGIN_SOURCES)
}

pub fn mcp_roots(home: &Path, workspace: Option<&Path>) -> Vec<SourceRoot> {
    let mut roots: Vec<SourceRoot> = MCP_SOURCES
        .iter()
        .flat_map(|source| {
            source.global_files.iter().map(move |file| SourceRoot {
                agent: source.agent,
                path: home.join(segments(file)),
                scope: SourceScope::User,
                format: source.format,
            })
        })
        .collect();
    if let Some(workspace) = workspace {
        roots.extend(MCP_SOURCES.iter().flat_map(|source| {
            source.project_files.iter().map(move |file| SourceRoot {
                agent: source.agent,
                path: workspace.join(segments(file)),
                scope: SourceScope::Workspace,
                format: source.format,
            })
        }));
    }
    roots
}

fn segments(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::new();
    for part in parts {
        path.push(part);
    }
    path
}
