//! MCP Apps runtime 状态（docs/specs/rust-mcp-ui.md）。
//!
//! 这里只保存**页面方向的 live-only 状态**：实例凭证、页面工具调用登记、App 工具登记与采样账本。
//! 不落库、不进 snapshot/replayable；MCP 资源与工具调用由 `Hub` 注入，模型调用由 Engine 注入。
mod instances;
mod resources;

pub(crate) use instances::{Binding, Instances, Lease, Scope, Source};
pub(crate) use resources::{bound_for_ui, project_resources, project_templates};

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// 页面调用登记的唯一 owner：`callId` 接纳后不可重放。
#[derive(Default)]
pub(crate) struct Calls {
    active: BTreeMap<String, CancellationToken>,
    finished: BTreeSet<String>,
}

fn call_key(session_id: &str, token: &str, call_id: &str) -> String {
    format!("{session_id}\u{0}{token}:{call_id}")
}

impl Calls {
    /// 接纳新调用；重复或已终结的 `callId` 一律拒绝（TS `Duplicate MCP App callId`）。
    pub(crate) fn begin(
        &mut self,
        session_id: &str,
        token: &str,
        call_id: &str,
    ) -> Result<CancellationToken> {
        let key = call_key(session_id, token, call_id);
        if self.active.contains_key(&key) || self.finished.contains(&key) {
            bail!("Duplicate MCP App callId");
        }
        let cancel = CancellationToken::new();
        self.active.insert(key, cancel.clone());
        Ok(cancel)
    }

    pub(crate) fn finish(&mut self, session_id: &str, token: &str, call_id: &str) {
        let key = call_key(session_id, token, call_id);
        if self.active.remove(&key).is_some() {
            self.finished.insert(key);
        }
    }

    /// 取消进行中的调用；未知 `callId` 记终态并回 false。
    pub(crate) fn cancel(&mut self, session_id: &str, token: &str, call_id: &str) -> bool {
        let key = call_key(session_id, token, call_id);
        let Some(controller) = self.active.remove(&key) else {
            self.finished.insert(key);
            return false;
        };
        self.finished.insert(key);
        controller.cancel();
        true
    }

    pub(crate) fn abort_session(&mut self, session_id: &str) {
        let prefix = format!("{session_id}\u{0}");
        self.abort_prefix(&prefix);
    }

    /// 实例撤销时只终结该实例的调用（`sessionId\0token:`）。
    pub(crate) fn abort_instance(&mut self, session_id: &str, token: &str) {
        let prefix = format!("{session_id}\u{0}{token}:");
        self.abort_prefix(&prefix);
    }

    fn abort_prefix(&mut self, prefix: &str) {
        let keys: Vec<String> = self
            .active
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(controller) = self.active.remove(&key) {
                controller.cancel();
            }
        }
        self.finished.retain(|key| !key.starts_with(prefix));
    }
}

#[derive(Clone)]
pub(crate) struct Subscriber {
    pub(crate) session_id: String,
    pub(crate) plugin_id: String,
    pub(crate) lease: Lease,
}

/// 资源订阅登记表：`(server, uri)` → 订阅者；引用计数决定何时向 MCP server 真正订阅。
#[derive(Default)]
pub(crate) struct Subscriptions {
    by_resource: BTreeMap<(String, String), Vec<Subscriber>>,
}

impl Subscriptions {
    /// 登记订阅；返回是否是该资源的第一位订阅者。
    pub(crate) fn add(&mut self, server: &str, uri: &str, subscriber: Subscriber) -> bool {
        let subscribers = self
            .by_resource
            .entry((server.to_owned(), uri.to_owned()))
            .or_default();
        let first = subscribers.is_empty();
        let duplicate = subscribers.iter().any(|current| {
            current.session_id == subscriber.session_id
                && current.lease.token == subscriber.lease.token
                && current.lease.generation == subscriber.lease.generation
        });
        if !duplicate {
            subscribers.push(subscriber);
        }
        first
    }

    /// 移除订阅；返回是否是该资源的最后一位订阅者（决定是否向 MCP server 退订）。
    pub(crate) fn remove(
        &mut self,
        server: &str,
        uri: &str,
        session_id: &str,
        token: &str,
        generation: u64,
    ) -> bool {
        let key = (server.to_owned(), uri.to_owned());
        let Some(subscribers) = self.by_resource.get_mut(&key) else {
            return false;
        };
        subscribers.retain(|current| {
            !(current.session_id == session_id
                && current.lease.token == token
                && current.lease.generation == generation)
        });
        let empty = subscribers.is_empty();
        if empty {
            self.by_resource.remove(&key);
        }
        empty
    }

    pub(crate) fn for_resource(&self, server: &str, uri: &str) -> Vec<Subscriber> {
        self.by_resource
            .get(&(server.to_owned(), uri.to_owned()))
            .cloned()
            .unwrap_or_default()
    }

    /// 该 server 下所有资源与订阅者（`list_changed` 通知用）。
    pub(crate) fn for_server(&self, server: &str) -> Vec<(String, Subscriber)> {
        self.by_resource
            .iter()
            .filter(|((candidate, _), _)| candidate == server)
            .flat_map(|((_, uri), subscribers)| {
                subscribers
                    .iter()
                    .cloned()
                    .map(move |subscriber| (uri.clone(), subscriber))
            })
            .collect()
    }

    pub(crate) fn remove_instance(&mut self, session_id: &str, token: &str, generation: u64) {
        for subscribers in self.by_resource.values_mut() {
            subscribers.retain(|current| {
                !(current.session_id == session_id
                    && current.lease.token == token
                    && current.lease.generation == generation)
            });
        }
        self.by_resource
            .retain(|_, subscribers| !subscribers.is_empty());
    }

    pub(crate) fn remove_session(&mut self, session_id: &str) {
        for subscribers in self.by_resource.values_mut() {
            subscribers.retain(|current| current.session_id != session_id);
        }
        self.by_resource
            .retain(|_, subscribers| !subscribers.is_empty());
    }
}

#[derive(Default)]
pub(crate) struct McpUi {
    pub(crate) instances: Instances,
    pub(crate) calls: Calls,
    pub(crate) subs: Subscriptions,
}

/// 协议 `escodeMcpUiPluginScopeShape` 的解析结果（owned，便于跨 await）。
pub(crate) struct PluginScope {
    pub(crate) workspace: Option<String>,
    pub(crate) session_id: String,
    pub(crate) plugin_id: String,
    pub(crate) server_name: String,
}

impl PluginScope {
    pub(crate) fn as_scope(&self) -> Scope<'_> {
        Scope {
            workspace: self.workspace.as_deref(),
            session_id: &self.session_id,
            plugin_id: &self.plugin_id,
            server_name: &self.server_name,
        }
    }
}

pub(crate) fn parse_scope(params: &Value) -> Result<PluginScope> {
    let text = |key: &str| -> Result<String> {
        params[key]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .with_context(|| format!("{key} is required"))
    };
    Ok(PluginScope {
        workspace: workspace_key(params),
        session_id: text("sessionId")?,
        plugin_id: text("pluginId")?,
        server_name: text("serverName")?,
    })
}

/// 身份键统一为 `workspaceIdentity?.trim() || workspacePath`（AGENTS.md Workspace Identity）。
pub(crate) fn workspace_key(params: &Value) -> Option<String> {
    let workspace = params.get("workspace")?;
    workspace["workspaceIdentity"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            workspace["workspacePath"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
}

pub(crate) fn parse_lease(params: &Value) -> Result<Lease> {
    let instance = params.get("instance").context("instance is required")?;
    let text = |key: &str| -> Result<String> {
        instance[key]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .with_context(|| format!("instance.{key} is required"))
    };
    Ok(Lease {
        runtime_id: text("runtimeId")?,
        generation: instance["generation"]
            .as_u64()
            .context("instance.generation is required")?,
        token: text("token")?,
        app_identity: text("appIdentity")?,
    })
}

pub(crate) fn parse_binding(params: &Value) -> Result<Binding> {
    let scope = parse_scope(params)?;
    let text = |key: &str| -> Result<String> {
        params[key]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .with_context(|| format!("{key} is required"))
    };
    Ok(Binding {
        workspace: scope.workspace.with_context(|| "workspace is required")?,
        account_context: params["accountContext"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        session_id: scope.session_id,
        plugin_id: scope.plugin_id,
        server_name: scope.server_name,
        scope_id: text("scopeId")?,
        resource_uri: text("resourceUri")?,
        owner_web_contents_id: params["ownerWebContentsId"]
            .as_u64()
            .context("ownerWebContentsId is required")?,
    })
}

#[cfg(test)]
#[path = "mcp_ui_tests.rs"]
mod tests;
