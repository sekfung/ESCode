//! 远端 scope 的 `window-controller`（docs/specs/source-server-remote-rust-host.md §10.13）。
//!
//! 任务事实 owner 是 Source Server 的 `escode-task`；本模块只维护**连接级**投影与订阅表，
//! 通过 `RemoteChannelClient` 读取/写入远端事实。远程失败显式返回，绝不回落本机 tasks-index。

#[path = "remote_listener.rs"]
mod remote_listener;
#[path = "remote_unread.rs"]
mod remote_unread;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use escode_host_control::logging;
use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::source_server_client::RemoteChannelClient;
use serde_json::{Value, json};

use super::list::{compare_items, decorate_search_item, projection_items};
use super::projection::Projection;
use super::rows::LocalScope;
use super::unread;
use crate::event_fanout::EventFanout;
use crate::remote_scope::RemoteScope;
use crate::session::RpcError;
use crate::wire::Wire;

/// 连接级远端 controller：一条远端 scope 连接一份投影与订阅表。
pub(crate) struct RemoteController {
    client: Arc<RemoteChannelClient>,
    scope: LocalScope,
    /// stable identity 是否解析出远端路径；解析失败时只允许读，写操作显式拒绝（spec §10.13）。
    workspace_path_valid: bool,
    projector: Arc<Projection>,
    dirty: AtomicBool,
    refresh_lock: tokio::sync::Mutex<()>,
    listener: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// `onDynamicTaskUnreadSignal` 的受众（远端 `escode-task` 事件派生，见 `unread.rs`）。
    unread: Arc<EventFanout>,
}

impl RemoteController {
    pub(super) fn new(client: Arc<RemoteChannelClient>, scope: &RemoteScope) -> Arc<Self> {
        let parsed = crate::remote_scope::remote_workspace_path(&scope.workspace_identity);
        let workspace_path = parsed.unwrap_or(&scope.workspace_identity).to_owned();
        let bound = LocalScope::remote(
            &workspace_path,
            &scope.workspace_identity,
            &scope.source_server_id,
        );
        let controller = Arc::new(Self {
            client,
            scope: bound,
            workspace_path_valid: parsed.is_some(),
            projector: Arc::new(Projection::new(Box::new(|| {
                uuid::Uuid::new_v4().to_string()
            }))),
            dirty: AtomicBool::new(false),
            refresh_lock: tokio::sync::Mutex::new(()),
            listener: Mutex::new(None),
            unread: Arc::new(EventFanout::default()),
        });
        controller.projector.register_source(&controller.scope);
        controller
    }

    pub(super) fn projector(&self) -> &Arc<Projection> {
        &self.projector
    }

    pub(super) fn unread(&self) -> &Arc<EventFanout> {
        &self.unread
    }

    fn scope_params(&self) -> Value {
        let mut params = json!({ "workspacePath": self.scope.workspace_path });
        if let Some(identity) = &self.scope.workspace_identity {
            params["workspaceIdentity"] = Value::from(identity.clone());
        }
        params
    }

    async fn call_json(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let arg = Some(WireValue::from(Wire::Array(vec![Wire::Object(params)])));
        self.client
            .call("escode-task", method, arg)
            .await
            .map(|value| wire_json(Wire::from(value)))
            .map_err(|error| {
                RpcError::new("Error", format!("escode-task.{method} failed: {error:#}"))
            })
    }

    /// TS `readSourceTaskIndex`：active/pinned/archived 三读合并，meta 归一为本连接 scope。
    async fn read_memberships(&self) -> Result<BTreeMap<String, Value>, RpcError> {
        let mut merged = BTreeMap::new();
        for (method, pinned, archived, active) in [
            ("listTasks", false, false, true),
            ("listPinnedTasks", true, false, true),
            ("listArchivedTasks", false, true, false),
        ] {
            let value = self.call_json(method, self.scope_params()).await?;
            let Some(items) = value.as_array() else {
                return Err(RpcError::new(
                    "Error",
                    format!("escode-task.{method} must return an array"),
                ));
            };
            for item in items {
                let mut meta = item.clone();
                super::normalize_scope(&mut meta, &self.scope);
                let task_id = meta["taskId"].as_str().unwrap_or_default().to_owned();
                if task_id.is_empty() {
                    continue;
                }
                merged.insert(
                    self.scope.row_key(&task_id),
                    json!({
                        "meta": meta,
                        "membership": {"pinned": pinned, "archived": archived, "active": active},
                    }),
                );
            }
        }
        Ok(merged)
    }

    /// single-flight + 脏标记：事件路径只置脏并后台续跑，显式读/写路径等待完成。
    pub(super) async fn refresh(&self, force: bool) -> Result<(), RpcError> {
        let _held = self.refresh_lock.lock().await;
        let key = self.scope.source_key();
        if !force && !self.dirty.swap(false, Ordering::SeqCst) && self.projector.refreshed(&key) {
            return Ok(());
        }
        match self.read_memberships().await {
            Ok(memberships) => {
                self.projector
                    .replace_source_snapshot(&self.scope, memberships, BTreeMap::new());
                Ok(())
            }
            Err(error) => {
                self.dirty.store(true, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    async fn refresh_after_write(&self) {
        if let Err(error) = self.refresh(true).await {
            logging::warn(
                "window-controller",
                format!(
                    "refresh after remote mutation failed for {}: {}",
                    self.scope.workspace_path, error.message
                ),
            );
        }
    }

    pub(super) async fn list_task_list(&self, query: Value) -> Result<Value, RpcError> {
        let key = self.scope.source_key();
        if let Err(error) = self.refresh(false).await {
            // 取过一次可信快照后，瞬时失败保留旧投影（TS `onSourceError`）；从未成功则显式报错。
            if !self.projector.refreshed(&key) {
                return Err(error);
            }
            logging::warn(
                "window-controller",
                format!(
                    "remote refresh failed for {}: {}",
                    self.scope.workspace_path, error.message
                ),
            );
        }
        let kind = query["kind"].as_str().unwrap_or("active");
        let sort_by = query["sortBy"].as_str().unwrap_or("updated");
        let limit = query["limit"].as_u64();
        let search = query["search"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut items = match search {
            // TS：单个 source 的搜索失败只记录并返回空，不猜数据。
            Some(_) => match self.search_items(&query).await {
                Ok(items) => items,
                Err(error) => {
                    logging::warn(
                        "window-controller",
                        format!(
                            "remote search failed for {}: {}",
                            self.scope.workspace_path, error.message
                        ),
                    );
                    Vec::new()
                }
            },
            None => projection_items(&self.projector, std::slice::from_ref(&self.scope), kind),
        };
        items.sort_by(|left, right| compare_items(left, right, sort_by));
        let total = items.len();
        let visible = match limit {
            Some(limit) => {
                let mut visible = items;
                visible.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
                visible
            }
            None => items,
        };
        Ok(json!({
            "items": visible,
            "total": total,
            "hasMore": total > visible.len(),
        }))
    }

    /// 搜索分支：远端 `escode-task.listTaskList` 收敛到本连接 scope，再与投影行叠加。
    async fn search_items(&self, query: &Value) -> Result<Vec<Value>, RpcError> {
        let mut remote = query.clone();
        remote["workspaceScopes"] = json!([self.scope_params()]);
        remote["limit"] = Value::Null;
        let value = self.call_json("listTaskList", remote).await?;
        let Some(items) = value["items"].as_array() else {
            return Ok(Vec::new());
        };
        Ok(items
            .iter()
            .map(|item| {
                let mut decorated = decorate_search_item(&self.projector, &self.scope, item);
                if let (Some(record), Some(source_server_id)) = (
                    decorated.as_object_mut(),
                    self.scope.source_server_id.as_ref(),
                ) {
                    record.insert(
                        "sourceServerId".to_owned(),
                        Value::from(source_server_id.clone()),
                    );
                }
                decorated
            })
            .collect())
    }

    fn validate(&self, address: &Value, mutation: &Value) -> Result<LocalScope, RpcError> {
        if !self.workspace_path_valid {
            return Err(RpcError::new(
                "Error",
                "远端 workspace identity 无法解析，禁止列表写操作",
            ));
        }
        let kind = mutation["kind"].as_str().unwrap_or_default();
        let allow_missing = matches!(kind, "delete-archived" | "delete-archived-batch");
        let scope = self.projector.validate_mutation(address, allow_missing)?;
        if scope.source_key() != self.scope.source_key() {
            return Err(RpcError::new("Error", "没有与任务地址匹配的 source"));
        }
        Ok(scope)
    }

    fn address_params(&self, address: &Value) -> Value {
        let mut params = json!({
            "taskId": address["taskId"].clone(),
            "workspacePath": address["workspacePath"].clone(),
        });
        if let Some(identity) = address["workspaceIdentity"].as_str() {
            params["workspaceIdentity"] = Value::from(identity);
        }
        params
    }

    async fn execute(&self, address: &Value, mutation: &Value) -> Result<(), RpcError> {
        let kind = mutation["kind"].as_str().unwrap_or_default();
        let mut params = self.address_params(address);
        match kind {
            "pin" => {
                params["pinned"] = Value::Bool(mutation["pinned"] == Value::Bool(true));
                self.call_json("setTaskPinned", params).await?;
            }
            "archive" => {
                let method = if mutation["archived"] == Value::Bool(true) {
                    "archiveTask"
                } else {
                    "unarchiveTask"
                };
                self.call_json(method, params).await?;
            }
            "delete" => {
                self.call_json("deleteTask", params).await?;
            }
            "mark-read" => {
                params["unread"] = Value::Bool(false);
                if let Some(expected) = mutation.get("expectedUnreadAt") {
                    params["expectedUnreadAt"] = expected.clone();
                }
                self.call_json("setTaskUnread", params).await?;
            }
            "mark-unread" => {
                params["unread"] = Value::Bool(true);
                self.call_json("setTaskUnread", params).await?;
            }
            "delete-archived" => {
                self.call_json("deleteArchivedTask", params).await?;
            }
            "delete-archived-batch" => {
                let mut batch = self.scope_params();
                batch["taskIds"] = mutation["taskIds"].clone();
                self.call_json("deleteArchivedTasks", batch).await?;
            }
            // TS：open/resume 在 controller 层是 no-op（只校验唯一 source）。
            "open" | "resume" => {}
            other => {
                return Err(RpcError::new(
                    "InvalidRequest",
                    format!("Unknown controller mutation `{other}`"),
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn mutate_task(
        &self,
        address: &Value,
        mutation: &Value,
    ) -> Result<Value, RpcError> {
        // TS 先 refreshSource（非强制）再校验：新任务/新 source 不能因为投影未读而误报缺行。
        self.refresh(false).await?;
        let task_id = address["taskId"].as_str().unwrap_or_default();
        if matches!(mutation["kind"].as_str(), Some("mark-read" | "mark-unread"))
            && self.projector.row(address, task_id).is_none()
        {
            self.refresh(true).await?;
        }
        self.validate(address, mutation)?;
        self.execute(address, mutation).await?;
        self.refresh_after_write().await;
        Ok(self
            .projector
            .row(address, task_id)
            .map(|row| row["meta"].clone())
            .unwrap_or(Value::Null))
    }

    pub(super) async fn delete_archived_task(&self, address: &Value) -> Result<Value, RpcError> {
        self.projector.validate_mutation(address, true)?;
        let value = self
            .call_json("deleteArchivedTask", self.address_params(address))
            .await?;
        self.refresh_after_write().await;
        Ok(value)
    }

    pub(super) async fn delete_archived_tasks(&self, params: &Value) -> Result<Value, RpcError> {
        let task_ids: Vec<Value> = params["taskIds"]
            .as_array()
            .map_or(Vec::new(), Clone::clone);
        if task_ids.is_empty() {
            return Ok(json!({
                "deletedTaskIds": [],
                "skippedTaskIds": [],
                "failedTaskIds": [],
            }));
        }
        let address = params["address"].clone();
        self.projector.validate_mutation(&address, true)?;
        let mut batch = self.scope_params();
        batch["taskIds"] = Value::Array(task_ids);
        let value = self.call_json("deleteArchivedTasks", batch).await?;
        self.refresh_after_write().await;
        Ok(value)
    }
}

fn wire_json(value: Wire) -> Value {
    match value {
        Wire::Undefined => Value::Null,
        Wire::Str(text) => Value::String(text),
        Wire::Int(number) => Value::from(number),
        Wire::Array(items) => Value::Array(items.into_iter().map(wire_json).collect()),
        Wire::Object(object) => object,
        // 任务面不返回二进制；保留 null 让 schema 校验显式失败而不是伪造字符串。
        Wire::Bytes { .. } => Value::Null,
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
