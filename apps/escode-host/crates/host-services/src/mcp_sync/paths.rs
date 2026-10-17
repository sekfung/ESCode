//! MCP 同步的目录描述符与配置文件路径（TS `mcpSyncService.ts` §descriptor）。
//!
//! 用户级：`.escode/cli/config.json`（键 `mcp.servers`）与 `.agents/mcp.json`（键 `mcpServers`）；
//! 工作区级：`<workspace>/.escode/config.json` 与 `<workspace>/.agents/mcp.json`。
//! `.escode` 存在有效 server 时优先，否则回退 `.agents`。

use std::path::PathBuf;

use serde_json::{Map, Value, json};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigKey {
    McpServers,
    Plain,
}

#[derive(Clone, Copy, Debug)]
pub struct Descriptor {
    pub source: &'static str,
    pub user_segments: &'static [&'static str],
    pub workspace_segments: &'static [&'static str],
    pub file_name: &'static str,
    pub config_key: ConfigKey,
}

pub const ESCODE: Descriptor = Descriptor {
    source: "escode",
    user_segments: &[".escode", "cli"],
    workspace_segments: &[".escode"],
    file_name: "config.json",
    config_key: ConfigKey::McpServers,
};

pub const AGENTS: Descriptor = Descriptor {
    source: "agents",
    user_segments: &[".agents"],
    workspace_segments: &[".agents"],
    file_name: "mcp.json",
    config_key: ConfigKey::Plain,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Workspace,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

pub fn config_path(
    descriptor: Descriptor,
    scope: Scope,
    home: &std::path::Path,
    workspace: Option<&str>,
) -> Result<PathBuf, String> {
    let (base, segments) = match scope {
        Scope::User => (home.to_path_buf(), descriptor.user_segments),
        Scope::Workspace => {
            let workspace = workspace.filter(|path| !path.is_empty()).ok_or_else(|| {
                format!(
                    "Missing workspace path for {} workspace MCP config",
                    descriptor.source
                )
            })?;
            (PathBuf::from(workspace), descriptor.workspace_segments)
        }
    };
    let mut path = base;
    for segment in segments {
        path.push(segment);
    }
    path.push(descriptor.file_name);
    Ok(path)
}

/// TS `buildDirectoryMcpLocation`：`projectPath` 只要传了 workspacePath 就带上（user 作用域也带）。
pub fn location(
    descriptor: Descriptor,
    scope: Scope,
    home: &std::path::Path,
    workspace: Option<&str>,
) -> Result<Value, String> {
    let path = config_path(descriptor, scope, home, workspace)?;
    let directory_path = path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut value = Map::new();
    value.insert("source".to_owned(), Value::from(descriptor.source));
    value.insert(
        "scope".to_owned(),
        Value::from(if scope == Scope::Workspace {
            "project"
        } else {
            "user"
        }),
    );
    value.insert("directoryPath".to_owned(), Value::from(directory_path));
    if let Some(workspace) = workspace.filter(|path| !path.is_empty()) {
        value.insert("projectPath".to_owned(), Value::from(workspace));
    }
    Ok(Value::Object(value))
}

/// `SettingsDirectoryLocation`（写面 payload / 读面记录）。
pub struct Location {
    pub source: String,
    pub scope: Scope,
    pub directory_path: String,
    pub project_path: Option<String>,
}

impl Location {
    pub fn from_json(value: &Value) -> Self {
        Self {
            source: value["source"].as_str().unwrap_or_default().to_owned(),
            scope: if value["scope"].as_str() == Some("project") {
                Scope::Workspace
            } else {
                Scope::User
            },
            directory_path: value["directoryPath"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            project_path: value["projectPath"].as_str().map(str::to_owned),
        }
    }
}

pub fn descriptor_for_source(source: &str) -> Result<Descriptor, String> {
    match source {
        "escode" => Ok(ESCODE),
        "agents" => Ok(AGENTS),
        other => Err(format!(
            "Unsupported MCP settings directory source: {other}"
        )),
    }
}

/// 读面记录里的 `file` 引用。
pub fn file_reference(path: &std::path::Path) -> Value {
    json!({"format": "json", "filePath": path.to_string_lossy()})
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
