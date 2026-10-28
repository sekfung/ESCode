//! `project` service channel：项目列表/树、保存/删除/排序、任务移动、目录解析与创建准备
//! （对齐上游 `project/index.ts` 的本地 Host 子集）。
//!
//! 远程 workspace 仍由 Node Host 承载（Q3），因此 hostId 固定 local；非 local 的请求显式
//! `project.host-offline`，不做静默降级。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::domains_project_session::{
    decorate_projects, prepare_session, read_directory, validate_move_task, validate_save_project,
    workspace_directory,
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

fn args_array(arg: Wire) -> Vec<Value> {
    match arg.into_args().into_iter().next().map(wire_value) {
        Some(Value::Array(values)) => values,
        _ => Vec::new(),
    }
}

fn arg_string(arg: Wire) -> String {
    match arg.into_args().into_iter().next().map(wire_value) {
        Some(Value::String(value)) => value,
        Some(Value::Object(object)) => object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// `project` channel。`data_base_dir` 为 `None` 时 `prepareSession` 显式失败（没有可分配的目录根）。
pub(crate) fn project_channel(
    read: ReadContext,
    home: PathBuf,
    data_base_dir: Option<PathBuf>,
) -> Channel {
    let mut methods: Vec<(&'static str, MethodHandler)> = Vec::new();
    let prepare_lock = Arc::new(Mutex::new(()));

    methods.push((
        "readDirectory",
        handler(move |arg| {
            let home = home.clone();
            Box::pin(async move {
                let params = args_object(arg);
                read_directory(&home, &params).await
            })
        }),
    ));

    methods.push((
        "readProjects",
        handler({
            let read = read.clone();
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let params = args_object(arg);
                    let include_deleted = params["includeDeleted"].as_bool().unwrap_or(false);
                    let mut projects = read
                        .run(move |index| {
                            index
                                .project_read_tree(include_deleted, false, false)
                                .map(|tree| {
                                    tree["projects"].as_array().cloned().unwrap_or_default()
                                })
                                .map_err(|error| error.to_string())
                        })
                        .await?;
                    decorate_projects(&mut projects).await;
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
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let params = args_object(arg);
                    let include_deleted = params["includeDeleted"].as_bool().unwrap_or(false);
                    let include_deleted_tasks =
                        params["includeDeletedTasks"].as_bool().unwrap_or(false);
                    let include_tasks = params["includeTasks"].as_bool().unwrap_or(true);
                    let mut tree = read
                        .run(move |index| {
                            index
                                .project_read_tree(
                                    include_deleted,
                                    include_deleted_tasks,
                                    include_tasks,
                                )
                                .map_err(|error| error.to_string())
                        })
                        .await?;
                    if let Some(projects) = tree["projects"].as_array_mut() {
                        decorate_projects(projects).await;
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
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = validate_save_project(&args_object(arg)).await?;
                    let project = read
                        .run(move |index| {
                            index
                                .project_save(&input)
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
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = json!({"id": arg_string(arg)});
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
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = args_object(arg);
                    validate_move_task(&input).await?;
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
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = json!({"ids": args_array(arg)});
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
            let lock = Arc::clone(&prepare_lock);
            move |arg| {
                let read = read.clone();
                let data_base_dir = data_base_dir.clone();
                let lock = Arc::clone(&lock);
                Box::pin(async move {
                    let Some(data_base_dir) = data_base_dir else {
                        return Err((
                            "TasksIndexUnavailable".to_owned(),
                            "prepareSession requires dataBaseDir".to_owned(),
                        ));
                    };
                    let params = args_object(arg);
                    prepare_session(&read, &workspace_directory(&data_base_dir), &params, &lock)
                        .await
                })
            }
        }),
    ));

    methods.push((
        "resolveSessionDirectories",
        handler(move |arg| {
            let read = read.clone();
            Box::pin(async move {
                let task = args_object(arg);
                let directories = read
                    .run(move |index| {
                        index
                            .project_resolve_directories(&task)
                            .map_err(|error| error.to_string())
                    })
                    .await?;
                Ok(Wire::Object(directories))
            })
        }),
    ));

    Channel::new("project", methods)
}
