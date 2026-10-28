//! `task-groups` service channel：任务分组的组织态读写（上游 `task-groups/index.ts` 的本地 Host 子集）。
//!
//! 与 `escode-task` 上的旧分组方法（`createTaskGroup` / `applyGroupedTaskViewOrder`）共用同一批
//! `task_group_*` 表；本 channel 只写组织事实，不触碰任务归属、会话事实与会话正文。

use serde_json::{Value, json};

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

fn args_string(arg: Wire) -> String {
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

fn rejected(code: &str) -> (String, String) {
    ("Rejected".to_owned(), code.to_owned())
}

/// 本地 Host 的引用校验：`hostId` 必须是 `local`，identity 不能是远端身份。
fn local_ref(task: &Value) -> bool {
    let host = task["hostId"].as_str().unwrap_or_default();
    let identity = task["workspaceIdentity"].as_str().unwrap_or_default();
    host == "local"
        && task["workspacePath"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
        && task["taskId"].as_str().is_some_and(|id| !id.is_empty())
        && !identity.starts_with("remote:")
}

fn same_task_key(left: &Value, right: &Value) -> bool {
    let key = |task: &Value| {
        let identity = task["workspaceIdentity"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| task["workspacePath"].as_str().unwrap_or_default());
        json!([identity, task["taskId"].as_str().unwrap_or_default()])
    };
    key(left) == key(right)
        && left["hostId"] == right["hostId"]
        && left["workspacePath"] == right["workspacePath"]
}

fn node_tasks(node: &Value, out: &mut Vec<Value>) {
    if node["type"] == "task" {
        out.push(node["task"].clone());
    }
}

pub(crate) fn task_groups_service_channel(read: ReadContext) -> Channel {
    let mut methods: Vec<(&'static str, MethodHandler)> = Vec::new();

    methods.push((
        "read",
        handler({
            let read = read.clone();
            move |_arg| {
                let read = read.clone();
                Box::pin(async move {
                    let structure = read
                        .run(move |index| {
                            index.task_group_read().map_err(|error| error.to_string())
                        })
                        .await?;
                    Ok(Wire::Object(structure))
                })
            }
        }),
    ));

    methods.push((
        "save",
        handler({
            let read = read.clone();
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = args_object(arg);
                    let group = read
                        .run(move |index| {
                            index
                                .task_group_save(&input)
                                .map_err(|error| error.to_string())
                        })
                        .await?;
                    Ok(Wire::Object(group))
                })
            }
        }),
    ));

    methods.push((
        "ungroup",
        handler({
            let read = read.clone();
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let id = args_string(arg);
                    read.run(move |index| {
                        index
                            .task_group_ungroup(&id)
                            .map_err(|error| error.to_string())
                    })
                    .await?;
                    Ok(Wire::Undefined)
                })
            }
        }),
    ));

    methods.push((
        "move",
        handler({
            let read = read.clone();
            move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = args_object(arg);
                    let mut refs = Vec::new();
                    node_tasks(&input["node"], &mut refs);
                    if !input["anchor"].is_null() {
                        node_tasks(&input["anchor"]["node"], &mut refs);
                    }
                    if refs.iter().any(|task| !local_ref(task)) {
                        return Err(rejected("task-group.host-identity-mismatch"));
                    }
                    let facts = read
                        .clone()
                        .run({
                            let refs = refs.clone();
                            move |index| {
                                index
                                    .task_group_resolve_facts(&refs)
                                    .map_err(|error| error.to_string())
                            }
                        })
                        .await?;
                    let available = refs.iter().all(|task| {
                        facts.iter().any(|fact| {
                            same_task_key(fact, task)
                                && fact["deleted"] != true
                                && fact["archived"] != true
                        })
                    });
                    if !available {
                        return Err(rejected("task-group.task-unavailable"));
                    }
                    read.run(move |index| {
                        index
                            .task_group_move(&input)
                            .map_err(|error| error.to_string())
                    })
                    .await?;
                    Ok(Wire::Undefined)
                })
            }
        }),
    ));

    methods.push((
        "admit",
        handler(move |arg| {
            let read = read.clone();
            Box::pin(async move {
                let input = args_object(arg);
                let refs: Vec<Value> = input["tasks"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(Value::is_object)
                    .collect();
                if refs.iter().any(|task| !local_ref(task)) {
                    return Err(rejected("task-group.host-identity-mismatch"));
                }
                let placement = input
                    .get("placement")
                    .filter(|value| !value.is_null())
                    .cloned();
                let placement_task = placement.as_ref().map(|value| value["task"].clone());
                if placement_task.as_ref().is_some_and(|task| !local_ref(task)) {
                    return Err(rejected("task-group.host-identity-mismatch"));
                }
                let facts = read
                    .clone()
                    .run({
                        let refs = refs.clone();
                        move |index| {
                            index
                                .task_group_resolve_facts(&refs)
                                .map_err(|error| error.to_string())
                        }
                    })
                    .await?;
                // 新建任务必须能在索引里看到且未删除，否则拒绝登记。
                if let Some(task) = &placement_task
                    && !facts
                        .iter()
                        .any(|fact| same_task_key(fact, task) && fact["deleted"] != true)
                {
                    return Err(rejected("task-group.created-task-unavailable"));
                }
                let mut payload = json!({"tasks": facts});
                if let Some(placement) = placement {
                    payload["placement"] = placement;
                }
                read.run(move |index| {
                    index
                        .task_group_admit(&payload)
                        .map_err(|error| error.to_string())
                })
                .await?;
                Ok(Wire::Undefined)
            })
        }),
    ));

    Channel::new("task-groups", methods)
}
