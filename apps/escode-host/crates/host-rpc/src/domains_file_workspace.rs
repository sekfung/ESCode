//! `file` 域的工作区创建类方法（TS `createDefaultWorkspace` / `createScratchWorkspace` /
//! `ensureConversationWorkspace`）。从 `domains_file.rs` 拆出以守住单文件行数上限。
//!
//! 两类根的语义不同（TS `paths.ts`）：
//! - 默认/命名工作区用 `homedir()`（`os.homedir()`，读进程 env HOME/USERPROFILE），
//!   即 `join(homedir(), "ESCodeProject", ...)`；
//! - 会话工作区用 `getConversationWorkspaceDir()` = `<dataBaseDir>/.escode/workspace/default`，
//!   dataBaseDir 由 Main 经控制消息 `rpcListen.dataBaseDir` 提交（Host 不读进程 env 推断）。

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

/// TS `ensureConversationWorkspace`：`<dataBaseDir>/.escode/workspace/default`。
///
/// `created` 对齐 Node `mkdir(path, {recursive:true})` 的返回值：叶子目录已存在时为 `undefined`
/// （`created=false`），本次建出叶子时为创建路径（`created=true`）。
/// mkdir 失败时按 TS 的 catch 分支处置：路径存在但不是目录 → 「not a directory」；否则原样报 mkdir 错误。
pub(crate) fn ensure_conversation_workspace(data_base_dir: &std::path::Path) -> ChannelResult {
    let path = data_base_dir.join(".escode").join("workspace").join("default");
    let existed = path.is_dir();
    if let Err(error) = std::fs::create_dir_all(&path) {
        // TS catch 分支：路径存在但不是目录 → 「not a directory」；否则原样报 mkdir 错误。
        if std::fs::metadata(&path).is_ok() {
            require_directory(&path)?;
        }
        return Err((
            "NodeIoError".to_owned(),
            node_error(&error, "mkdir", &path.to_string_lossy()),
        ));
    }
    require_directory(&path)?;
    Ok(Wire::Object(json!({
        "path": path.to_string_lossy(),
        "created": !existed,
        "workspacePurpose": "conversation",
    })))
}

/// `mkdir -p` 语义 + 必须是目录（Node `{recursive:true}` 对已存在目录幂等成功）。
fn ensure_workspace_dir(path: &std::path::Path) -> Result<(), (String, String)> {
    if let Err(error) = std::fs::create_dir_all(path) {
        return Err((
            "NodeIoError".to_owned(),
            node_error(&error, "mkdir", &path.to_string_lossy()),
        ));
    }
    require_directory(path)
}

/// 目录断言：失败文案与 TS `new Error("Workspace path is not a directory: ...")` 一致。
fn require_directory(path: &std::path::Path) -> Result<(), (String, String)> {
    let is_dir = std::fs::metadata(path).is_ok_and(|meta| meta.is_dir());
    if !is_dir {
        return Err((
            "InvalidRequest".to_owned(),
            format!("Workspace path is not a directory: {}", path.display()),
        ));
    }
    Ok(())
}
