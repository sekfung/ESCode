//! `subagents` 的路径解析（TS `subagentStorage.ts`）。
//!
//! 与 `commands` 的区别：这里解析到的是**配置存储根本身**（默认 `~/.escode`），不是 `<root>/cli`。
//! 用户 agent 根 = `<storageRoot>/agents`；状态文件 = `<storageRoot>/v2/agents-state.json`。

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

const HOME_PREFIX: &str = "~/";
const DEFAULT_STORAGE_DIR: &str = "~/.escode";

/// TS `readUserCliConfig`：`<home>/.escode/cli/config.json`，坏输入当空对象。
pub async fn read_user_cli_config(home: &Path) -> Value {
    let path = home.join(".escode").join("cli").join("config.json");
    let Ok(raw) = tokio::fs::read_to_string(path).await else {
        return Value::Object(Default::default());
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) if value.is_object() => value,
        _ => Value::Object(Default::default()),
    }
}

fn is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// TS `resolveConfigPath`：`~/` 展开到 home；绝对路径原样；其余按进程 cwd。
pub fn resolve_config_path(home: &Path, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix(HOME_PREFIX) {
        return normalize_lexical(&home.join(rest));
    }
    if is_absolute(path) {
        return normalize_lexical(Path::new(path));
    }
    match escode_cli_core_api::env::current_dir() {
        Ok(cwd) => normalize_lexical(&cwd.join(path)),
        Err(_) => normalize_lexical(Path::new(path)),
    }
}

/// TS `resolveESCodeStorageRoot`：`config.storage.dir`（trim 后非空）否则 `~/.escode`。
pub async fn storage_root(home: &Path) -> PathBuf {
    let config = read_user_cli_config(home).await;
    let storage_dir = config
        .get("storage")
        .and_then(|storage| storage.get("dir"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .unwrap_or(DEFAULT_STORAGE_DIR);
    resolve_config_path(home, storage_dir)
}

/// TS `resolveUserSubagentRoot`。
pub async fn user_agents_root(home: &Path) -> PathBuf {
    storage_root(home).await.join("agents")
}

/// TS `resolveWorkspaceSubagentRoot`。
pub fn workspace_agents_root(workspace_path: &str) -> PathBuf {
    PathBuf::from(workspace_path).join(".escode").join("agents")
}

/// TS `resolveSubagentStateFile`。
pub async fn state_file(home: &Path) -> PathBuf {
    storage_root(home)
        .await
        .join("v2")
        .join("agents-state.json")
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
