//! `project` service channel：项目列表/树、保存/删除/排序、任务移动、目录解析与创建准备。
//!
//! S5-e：Rust Host 同时持有本地与远端 owner 的项目容器（`projects.host_id` 区分），
//! 目录存在性/列目录按 owner 路由到在线 Source 的 `file` channel；离线显式
//! `project.host-offline`，不静默回退本地文件系统。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use escode_host_services::source_server_connections::SourceServerConnections;

use crate::domains_project_remote::{
    DirectoryProbe, RemoteHost, resolve_remote, workspace_identity_for_host,
};
use crate::domains_project_session::{
    decorate_projects, is_absolute, normalize_path, prepare_session, read_directory,
    validate_move_task, validate_save_project, workspace_directory,
};
use crate::domains_task_support::ReadContext;
use crate::session::{Channel, MethodHandler, handler};
use crate::wire::Wire;

fn wire_value(wire: Wire) -> Value {
    match wire {
        Wire::Undefined => Value::Null,
        Wire::Str(value) => json!(value),
        Wire::Int(value) => json!(value),
        Wire::Object(value) => value,
        Wire::Array(values) => Value::Array(values.into_iter().map(wire_value).collect()),
        Wire::Bytes { data, .. } => json!(data),
    }
}

fn args_object(arg: Wire) -> Value {
    arg.into_args()
        .into_iter()
        .next()
        .map(wire_value)
        .unwrap_or(Value::Null)
}

/// `project` channel。`data_base_dir` 为 `None` 时 `prepareSession` 显式失败（没有可分配的目录根）。
pub(crate) fn project_channel(
    read: ReadContext,
    home: PathBuf,
    data_base_dir: Option<PathBuf>,
    connections: Arc<SourceServerConnections>,
) -> Channel {
    let mut methods: Vec<(&'static str, MethodHandler)> = Vec::new();
    let prepare_lock = Arc::new(Mutex::new(()));

    methods.push((
        "readDirectory",
        handler({
            let connections = Arc::clone(&connections);
            move |arg| {
                let connections = Arc::clone(&connections);
                let home = home.clone();
                Box::pin(async move {
                    let params = args_object(arg);
                    let host_id = params_host_id(&params).to_owned();
                    let remote = resolve_remote(&connections, &host_id).map_err(db_error)?;
                    let Some(remote) = remote else {
                        return read_directory(&home, &params).await;
                    };
                    let requested = match params.get("path") {
                        None | Some(Value::Null) => remote.home().await.map_err(db_error)?,
                        Some(Value::String(path)) if !path.is_empty() => path.clone(),
                        Some(_) => return Err(rejected("project.invalid-request")),
                    };
                    if !is_absolute(&requested) {
                        return Err(rejected("project.absolute-path-required"));
                    }
                    let path = normalize_path(&requested);
                    remote
                        .readdir(&path, params["includeHidden"].as_bool())
                        .await
                        .map_err(db_error)
                })
            }
        }),
    ));

    methods.push((
        "readProjects",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                Box::pin(async move {
                    let params = args_object(arg);
                    let host_id = params_host_id(&params).to_owned();
                    let include_deleted = params["includeDeleted"].as_bool().unwrap_or(false);
                    let mut projects = {
                        let host_id = host_id.clone();
                        read.run(move |index| {
                            index
                                .project_read_projects(include_deleted, &host_id)
                                .map_err(|error| error.to_string())
                        })
                        .await?
                    };
                    let remote = resolve_remote(&connections, &host_id).map_err(db_error)?;
                    let probe = probe_for(&remote);
                    decorate_projects(&mut projects, &probe).await;
                    Ok(Wire::Array(
                        projects.into_iter().map(Wire::Object).collect(),
                    ))
                })
            }
        }),
    ));

    methods.push((
        "readTree",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                Box::pin(async move {
                    let params = args_object(arg);
                    let host_id = params_host_id(&params).to_owned();
                    let include_deleted = params["includeDeleted"].as_bool().unwrap_or(false);
                    let include_deleted_tasks =
                        params["includeDeletedTasks"].as_bool().unwrap_or(false);
                    let include_tasks = params["includeTasks"].as_bool().unwrap_or(true);
                    let mut tree = {
                        let host_id = host_id.clone();
                        read.run(move |index| {
                            index
                                .project_read_tree(
                                    include_deleted,
                                    include_deleted_tasks,
                                    include_tasks,
                                    &host_id,
                                )
                                .map_err(|error| error.to_string())
                        })
                        .await?
                    };
                    let remote = resolve_remote(&connections, &host_id).map_err(db_error)?;
                    let probe = probe_for(&remote);
                    if let Some(projects) = tree["projects"].as_array_mut() {
                        decorate_projects(projects, &probe).await;
                    }
                    Ok(Wire::Object(tree))
                })
            }
        }),
    ));

    methods.push((
        "saveProject",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                Box::pin(async move {
                    let input = args_object(arg);
                    let host_id = params_host_id(&input).to_owned();
                    let remote = resolve_remote(&connections, &host_id).map_err(db_error)?;
                    let probe = probe_for(&remote);
                    let validated = validate_save_project(&input, &probe).await?;
                    let project = read
                        .run(move |index| {
                            index
                                .project_save(&validated)
                                .map_err(|error| error.to_string())
                        })
                        .await?;
                    Ok(Wire::Object(project))
                })
            }
        }),
    ));

    methods.push((
        "deleteProject",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                Box::pin(async move {
                    let values = arg_values(arg);
                    let id = values
                        .first()
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let host_id = values
                        .get(1)
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("local")
                        .to_owned();
                    require_online_if_remote(&connections, &host_id)?;
                    let input = json!({"id": id, "hostId": host_id});
                    read.run(move |index| {
                        index
                            .project_delete(&input)
                            .map_err(|error| error.to_string())
                    })
                    .await?;
                    Ok(Wire::Undefined)
                })
            }
        }),
    ));

    methods.push((
        "moveTask",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                Box::pin(async move {
                    let input = args_object(arg);
                    let host_id = params_host_id(&input).to_owned();
                    let remote = resolve_remote(&connections, &host_id).map_err(db_error)?;
                    let probe = probe_for(&remote);
                    validate_move_task(&input, &probe).await?;
                    read.run(move |index| {
                        index
                            .project_move_task(&input)
                            .map_err(|error| error.to_string())
                    })
                    .await?;
                    Ok(Wire::Undefined)
                })
            }
        }),
    ));

    methods.push((
        "reorderProjects",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                Box::pin(async move {
                    let values = arg_values(arg);
                    let ids = match values.first() {
                        Some(Value::Array(ids)) => ids.clone(),
                        _ => Vec::new(),
                    };
                    let host_id = values
                        .get(1)
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("local")
                        .to_owned();
                    require_online_if_remote(&connections, &host_id)?;
                    let input = json!({"ids": ids, "hostId": host_id});
                    read.run(move |index| {
                        index
                            .project_reorder(&input)
                            .map_err(|error| error.to_string())
                    })
                    .await?;
                    Ok(Wire::Undefined)
                })
            }
        }),
    ));

    methods.push((
        "prepareSession",
        handler({
            let read = read.clone();
            let connections = Arc::clone(&connections);
            let lock = Arc::clone(&prepare_lock);
            move |arg| {
                let read = read.clone();
                let connections = Arc::clone(&connections);
                let data_base_dir = data_base_dir.clone();
                let lock = Arc::clone(&lock);
                Box::pin(async move {
                    let params = args_object(arg);
                    let host_id = params_host_id(&params).to_owned();
                    let remote = resolve_remote(&connections, &host_id).map_err(db_error)?;
                    let probe = probe_for(&remote);
                    // 本地需要可分配的目录根；远端 owner 的目录由项目 root 固定，不本地分配。
                    let workspace_dir = match (&remote, data_base_dir) {
                        (_, Some(base)) => workspace_directory(&base),
                        (Some(_), None) => PathBuf::new(),
                        (None, None) => {
                            return Err((
                                "TasksIndexUnavailable".to_owned(),
                                "prepareSession requires dataBaseDir".to_owned(),
                            ));
                        }
                    };
                    prepare_session(&read, &workspace_dir, &params, &lock, &probe).await
                })
            }
        }),
    ));

    methods.push((
        "resolveSessionDirectories",
        handler({
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let task = args_object(arg);
                    let host_id = params_host_id(&task).to_owned();
                    let task_identity = task["workspaceIdentity"]
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                    let mut directories = read
                        .run(move |index| {
                            index
                                .project_resolve_directories(&task)
                                .map_err(|error| error.to_string())
                        })
                        .await?;
                    if host_id != "local"
                        && let Some(path) = directories["workspacePath"].as_str()
                    {
                        let identity = task_identity
                            .unwrap_or_else(|| workspace_identity_for_host(&host_id, path));
                        directories["workspaceIdentity"] = json!(identity);
                    }
                    Ok(Wire::Object(directories))
                })
            }
        }),
    ));

    Channel::new("project", methods)
}

fn params_host_id(params: &Value) -> &str {
    params["hostId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or("local")
}

fn probe_for(remote: &Option<RemoteHost>) -> DirectoryProbe<'_> {
    match remote {
        Some(host) => DirectoryProbe::Remote(host),
        None => DirectoryProbe::Local,
    }
}

/// 只依赖 DB 的写操作仍要求远端在线：离线时不能在没有 owner 的情况下改归属。
fn require_online_if_remote(
    connections: &SourceServerConnections,
    host_id: &str,
) -> Result<(), (String, String)> {
    resolve_remote(connections, host_id).map(|_| ())
}

fn db_error(error: (String, String)) -> (String, String) {
    error
}

fn rejected(code: &str) -> (String, String) {
    ("Rejected".to_owned(), code.to_owned())
}

fn arg_values(arg: Wire) -> Vec<Value> {
    match arg {
        Wire::Array(values) => values.into_iter().map(wire_value).collect(),
        other => vec![wire_value(other)],
    }
}

#[cfg(test)]
#[path = "domains_project_tests.rs"]
mod tests;
