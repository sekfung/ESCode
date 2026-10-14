//! `file` 域的工作区创建类方法（TS `createDefaultWorkspace` / `createScratchWorkspace`）。
//! 从 `domains_file.rs` 拆出以守住单文件行数上限。
//!
//! 未实现：`ensureConversationWorkspace`——它依赖 Host 侧 dataBaseDir 配置
//! （`setDataBaseDir()` > `ESCODE_DATA_BASE_DIR` > HOME，且可被设置文件的 `dataBaseDir` 覆盖），
//! 属 Host 路径配置子系统，登记为后续子项。

use serde_json::{Value, json};

use crate::domains_file::node_error;
use crate::session::ChannelResult;
use crate::wire::Wire;

/// TS `SCRATCH_WORKSPACE_ROOT_NAME`：与 homedir 拼接出默认工作区根。
const SCRATCH_WORKSPACE_ROOT_NAME: &str = "ESCodeProject";

/// TS `createDefaultWorkspace`：`join(homedir(), "ESCodeProject")`，mkdir recursive 后必须是目录。
pub(crate) fn create_default_workspace(home: &std::path::Path) -> ChannelResult {
    let path = home.join(SCRATCH_WORKSPACE_ROOT_NAME);
    ensure_workspace_dir(&path)?;
    Ok(Wire::Object(json!({"path": path.to_string_lossy()})))
}

/// TS `createScratchWorkspace`：名称校验 + `join(homedir(), "ESCodeProject", name)`。
pub(crate) fn create_scratch_workspace(home: &std::path::Path, params: &Value) -> ChannelResult {
    let name = params["name"].as_str().unwrap_or_default();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err((
            "InvalidRequest".to_owned(),
            "Workspace name is required.".to_owned(),
        ));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err((
            "InvalidRequest".to_owned(),
            "Workspace name cannot contain path separators.".to_owned(),
        ));
    }
    let path = home.join(SCRATCH_WORKSPACE_ROOT_NAME).join(trimmed);
    ensure_workspace_dir(&path)?;
    Ok(Wire::Object(json!({"path": path.to_string_lossy()})))
}

/// `mkdir -p` 语义 + 必须是目录（Node `{recursive:true}` 对已存在目录幂等成功）。
fn ensure_workspace_dir(path: &std::path::Path) -> Result<(), (String, String)> {
    if let Err(error) = std::fs::create_dir_all(path) {
        return Err((
            "NodeIoError".to_owned(),
            node_error(&error, "mkdir", &path.to_string_lossy()),
        ));
    }
    let is_dir = std::fs::metadata(path).is_ok_and(|meta| meta.is_dir());
    if !is_dir {
        return Err((
            "InvalidRequest".to_owned(),
            format!("Workspace path is not a directory: {}", path.display()),
        ));
    }
    Ok(())
}
