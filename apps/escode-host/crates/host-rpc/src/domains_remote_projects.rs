//! `remote-projects` channel：本机保存的远端项目入口（W3 / 上游 `remote-projects/index.ts`）。
//!
//! 唯一权威是本机 `tasks-index.sqlite` 的 `hosts` / `remote_project_entries` 表；
//! 连接运行态从 Source registry 读取，不落库、不另建状态机。写方法成功即代表已提交。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};

use escode_host_services::source_server::SourceServerRegistry;
use escode_host_services::source_server_connections::{ConnectIntent, SourceServerConnections};

use crate::domains_task_support::ReadContext;
use crate::remote_host_id::{host_id_for_source, source_id_from_host_id};
use crate::session::{Channel, MethodHandler, RpcError, RpcResult, handler_rpc};
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

fn args(arg: Wire) -> Vec<Value> {
    arg.into_args().into_iter().map(wire_value).collect()
}

fn object_arg(arg: Wire) -> Value {
    args(arg).into_iter().next().unwrap_or(Value::Null)
}

fn string_arg(arg: Wire) -> String {
    match args(arg).into_iter().next() {
        Some(Value::String(value)) => value,
        Some(Value::Object(object)) => object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

fn db_error(error: (String, String)) -> RpcError {
    RpcError::new(error.0, error.1)
}

fn status_value(state: &str) -> &'static str {
    match state {
        "online" => "online",
        "bootstrapping" | "retrying" | "updating" => "connecting",
        _ => "offline",
    }
}

/// 连接运行态投影：hostId → 当前 preferred/首个 profile 的状态。
fn connection_states(registry: &SourceServerRegistry) -> BTreeMap<String, Value> {
    let Ok(snapshot) = registry.snapshot() else {
        return BTreeMap::new();
    };
    let servers = snapshot["servers"].as_array().cloned().unwrap_or_default();
    let states = snapshot["states"].as_array().cloned().unwrap_or_default();
    let mut result = BTreeMap::new();
    for state in states {
        let Some(source_id) = state["sourceServerId"].as_str() else {
            continue;
        };
        let host_id = host_id_for_source(source_id);
        let preferred = servers
            .iter()
            .find(|server| server["sourceServerId"] == json!(source_id))
            .and_then(|server| server["preferredProfileId"].as_str());
        let profile_id = state["profileId"].as_str().unwrap_or_default();
        let is_preferred = preferred == Some(profile_id);
        let value = status_value(state["state"].as_str().unwrap_or_default());
        let existing = result.get(&host_id);
        let existing_online =
            existing.is_some_and(|current: &Value| current["connectionStatus"] == json!("online"));
        let should_replace = existing.is_none() || (is_preferred && !existing_online);
        if should_replace {
            let mut projection = json!({ "connectionStatus": value });
            if !state["error"].is_null() {
                projection["lastConnectionError"] = state["error"].clone();
            }
            if !state["state"].is_null() {
                projection["lastConnectionStatus"] = state["state"].clone();
            }
            if !state["upgradeTarget"].is_null() {
                projection["upgradeTarget"] = state["upgradeTarget"].clone();
            }
            result.insert(host_id, projection);
        }
    }
    result
}

fn decorate_directory(mut directory: Value, registry: &SourceServerRegistry) -> Value {
    let states = connection_states(registry);
    if let Some(hosts) = directory["hosts"].as_array_mut() {
        for host in hosts {
            let Some(host_id) = host["id"].as_str() else {
                continue;
            };
            match states.get(host_id) {
                Some(state) => {
                    host["connectionStatus"] = state["connectionStatus"].clone();
                    for key in [
                        "lastConnectionError",
                        "lastConnectionStatus",
                        "upgradeTarget",
                    ] {
                        if !state[key].is_null() {
                            host[key] = state[key].clone();
                        }
                    }
                }
                None => host["connectionStatus"] = json!("offline"),
            }
        }
    }
    directory
}

async fn find_host(read: &ReadContext, id: &str) -> Result<Option<Value>, RpcError> {
    let directory = read
        .run(|index| {
            index
                .remote_project_read_directory()
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(db_error)?;
    Ok(directory["hosts"]
        .as_array()
        .and_then(|hosts| hosts.iter().find(|host| host["id"] == json!(id)).cloned()))
}

async fn connect_host(
    read: &ReadContext,
    registry: &SourceServerRegistry,
    connections: &SourceServerConnections,
    id: &str,
) -> RpcResult {
    let host = find_host(read, id)
        .await?
        .ok_or_else(|| RpcError::new("Rejected", "remote-project.host-not-found"))?;
    let source_id = source_id_from_host_id(id)
        .ok_or_else(|| RpcError::new("Rejected", "remote-project.invalid-host-identity"))?
        .to_owned();
    let preferred = registry
        .source_server(&source_id)
        .and_then(|server| server.preferred_profile_id);
    let profile_id = registry
        .profile_ids_for_source(&source_id)
        .into_iter()
        .find(|profile_id| {
            registry
                .profile(profile_id)
                .is_some_and(|profile| profile.lifecycle_state != "retired")
                && preferred.as_deref() == Some(profile_id.as_str())
        })
        .or_else(|| {
            registry
                .profile_ids_for_source(&source_id)
                .into_iter()
                .find(|profile_id| {
                    registry
                        .profile(profile_id)
                        .is_some_and(|profile| profile.lifecycle_state != "retired")
                })
        })
        .ok_or_else(|| RpcError::new("Rejected", "remote-project.profile-not-found"))?;
    connections
        .connect_with(&profile_id, ConnectIntent::Recover)
        .await
        .map_err(|error| RpcError::new("Error", format!("{error:#}")))?;
    let online = registry
        .connection_state(&profile_id)
        .is_some_and(|state| state.state == "online");
    if !online {
        return Err(RpcError::new("Error", "project.host-offline"));
    }
    let mut host = host;
    host["connectionStatus"] = json!("online");
    Ok(Wire::Object(host))
}

/// `remote-projects` channel；`read` 未装配任务库时所有方法显式失败（不改回内存态）。
pub(crate) fn remote_projects_channel(
    read: ReadContext,
    registry: Arc<SourceServerRegistry>,
    connections: Arc<SourceServerConnections>,
) -> Channel {
    let mut methods: Vec<(&'static str, MethodHandler)> = Vec::new();
    {
        let read = read.clone();
        let registry_for_read = Arc::clone(&registry);
        methods.push((
            "readDirectory",
            handler_rpc(move |_arg| {
                let read = read.clone();
                let registry = Arc::clone(&registry_for_read);
                Box::pin(async move {
                    let directory = read
                        .run(|index| {
                            index
                                .remote_project_read_directory()
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(db_error)?;
                    Ok(Wire::Object(decorate_directory(directory, &registry)))
                })
            }),
        ));
    }
    {
        let read = read.clone();
        methods.push((
            "saveHost",
            handler_rpc(move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = object_arg(arg);
                    let host = read
                        .run(move |index| {
                            index
                                .remote_project_save_host(&input)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(db_error)?;
                    Ok(Wire::Object(host))
                })
            }),
        ));
    }
    {
        let read = read.clone();
        methods.push((
            "bindHost",
            handler_rpc(move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = object_arg(arg);
                    let host = read
                        .run(move |index| {
                            index
                                .remote_project_bind_host(&input)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(db_error)?;
                    Ok(Wire::Object(host))
                })
            }),
        ));
    }
    {
        let read = read.clone();
        methods.push((
            "removeHost",
            handler_rpc(move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let id = string_arg(arg);
                    read.run(move |index| {
                        index
                            .remote_project_remove_host(&id)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(db_error)?;
                    Ok(Wire::Undefined)
                })
            }),
        ));
    }
    {
        let read = read.clone();
        methods.push((
            "addEntry",
            handler_rpc(move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = object_arg(arg);
                    let entry = read
                        .run(move |index| {
                            index
                                .remote_project_add_entry(&input)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(db_error)?;
                    Ok(Wire::Object(entry))
                })
            }),
        ));
    }
    {
        let read = read.clone();
        methods.push((
            "refreshEntry",
            handler_rpc(move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let input = object_arg(arg);
                    let applied = read
                        .run(move |index| {
                            index
                                .remote_project_refresh_entry(&input)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(db_error)?;
                    Ok(Wire::Object(Value::Bool(applied)))
                })
            }),
        ));
    }
    {
        let read = read.clone();
        methods.push((
            "removeEntry",
            handler_rpc(move |arg| {
                let read = read.clone();
                Box::pin(async move {
                    let id = string_arg(arg);
                    read.run(move |index| {
                        index
                            .remote_project_remove_entry(&id)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(db_error)?;
                    Ok(Wire::Undefined)
                })
            }),
        ));
    }
    let registry_for_connect = Arc::clone(&registry);
    let connections_for_connect = Arc::clone(&connections);
    methods.push((
        "connectHost",
        handler_rpc(move |arg| {
            let read = read.clone();
            let registry = Arc::clone(&registry_for_connect);
            let connections = Arc::clone(&connections_for_connect);
            Box::pin(async move {
                let id = string_arg(arg);
                connect_host(&read, &registry, &connections, &id).await
            })
        }),
    ));
    Channel::new("remote-projects", methods)
}
