//! `memory` channel（docs/specs/rust-u2-user-config-services.md §2）：用户级 `MEMORY.md` 与只读的 Project Memory 视图。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::channel::Channel;
use crate::domains_file::node_error;
use crate::project_memory;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

/// TS `getMemoryFileName`：目前只有 `MEMORY.md`。
const USER_MEMORY_FILE: &str = "MEMORY.md";

#[derive(Clone)]
struct Roots {
    user_dir: PathBuf,
    projects: Option<PathBuf>,
}

fn params_of(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

fn io_error(error: &std::io::Error, op: &str, path: &Path) -> RpcError {
    RpcError::new(
        "NodeIoError",
        node_error(error, op, &path.to_string_lossy()),
    )
}

fn gap() -> RpcError {
    RpcError::new(
        "NotImplemented",
        "memory project views need dataBaseDir: Main must pass dataBaseDir in rpcListen.",
    )
}

fn method<F, Fut>(roots: &Roots, run: F) -> MethodHandler
where
    F: Fn(Roots, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Wire, RpcError>> + Send + 'static,
{
    let roots = roots.clone();
    let run = Arc::new(run);
    handler_rpc(move |arg: Wire| {
        let (roots, run) = (roots.clone(), Arc::clone(&run));
        Box::pin(async move { run(roots, params_of(arg)).await })
    })
}

/// 在阻塞线程里跑同步 FS 逻辑（Project Memory 的句柄校验需要同步语义）。
async fn blocking<T: Send + 'static>(
    run: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| RpcError::new("Fatal", error.to_string()))?
}

pub(crate) fn memory_channel(home: &Path, data_base_dir: Option<&Path>) -> Channel {
    let roots = Roots {
        user_dir: home.join(".claude").join("memory"),
        projects: data_base_dir.map(project_memory::projects_root),
    };
    Channel::new(
        "memory",
        vec![
            (
                "loadMemory",
                method(&roots, |roots, _| async move {
                    let path = roots.user_dir.join(USER_MEMORY_FILE);
                    // 任何读取失败（含不存在）都回 null，与 TS 一致；非 UTF-8 字节按 Node 的替换字符解码。
                    let memory = tokio::fs::read(&path)
                        .await
                        .ok()
                        .map(|bytes| json!({"content": String::from_utf8_lossy(&bytes), "enabled": true}));
                    Ok(Wire::Object(json!({"memory": memory})))
                }),
            ),
            (
                "saveMemory",
                method(&roots, |roots, params| async move {
                    tokio::fs::create_dir_all(&roots.user_dir)
                        .await
                        .map_err(|e| io_error(&e, "mkdir", &roots.user_dir))?;
                    let path = roots.user_dir.join(USER_MEMORY_FILE);
                    let content = params["config"]["content"].as_str().unwrap_or_default();
                    tokio::fs::write(&path, content)
                        .await
                        .map_err(|e| io_error(&e, "open", &path))?;
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "clearMemory",
                method(&roots, |roots, _| async move {
                    // 写不进去（如目录不存在）按 TS 静默忽略。
                    let _ = tokio::fs::write(roots.user_dir.join(USER_MEMORY_FILE), "").await;
                    Ok(Wire::Undefined)
                }),
            ),
            (
                "getUserMemoryDirectory",
                method(&roots, |roots, _| async move {
                    let _ = tokio::fs::create_dir_all(&roots.user_dir).await;
                    Ok(Wire::Object(
                        json!({"path": roots.user_dir.to_string_lossy()}),
                    ))
                }),
            ),
            (
                "listProjectMemories",
                method(&roots, |roots, _| async move {
                    let root = roots.projects.ok_or_else(gap)?;
                    blocking(move || project_memory::list(&root))
                        .await
                        .map(Wire::Object)
                }),
            ),
            (
                "readProjectMemoryFile",
                method(&roots, |roots, params| async move {
                    let root = roots.projects.ok_or_else(gap)?;
                    let workspace = params["workspaceId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let file = params["fileName"].as_str().unwrap_or_default().to_owned();
                    blocking(move || project_memory::read_file(&root, &workspace, &file))
                        .await
                        .map(Wire::Object)
                }),
            ),
        ],
    )
}
