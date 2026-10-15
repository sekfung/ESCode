//! `window-controller` channel（docs/specs/rust-u2-window-controller.md）：窗口级列表投影 + 跨 source 路由。
//!
//! 本片只做**本地 source**（远程 source 见 §5-1 登记差异）。状态所有者：
//! tasks-index（唯一写者）→ 本模块的刷新器 → [`projection::Projection`]（rows/topic 水位/订阅）→
//! 连接作用域的 `EventFire` 帧。刷新触发：`listTaskList`/mutation（显式，等待完成）、
//! `WorkspaceEvents` 索引写事件、syncer 会话摘要变化——后两者只置脏标记 + 后台刷新（合并式）。
//!
//! TS 真相源：`packages/desktop/src/host/windowHostControllerService.ts`（source 聚合与刷新）、
//! `windowHostControllerProjection.ts`（纯投影）、`windowHostSessionsIndexObserver.ts`（摘要观察）。

mod controller;
mod frames;
mod list;
mod mutate;
mod overlays;
mod projection;
mod rows;
mod scope;

use std::sync::Arc;

use crate::domains_task_support::first_object;
use crate::session::{
    Channel, ChannelFactory, ConnectionContext, EventHandler, MethodHandler, RpcError, handler_rpc,
};
use crate::wire::Wire;

pub(crate) use controller::WindowController;
pub(crate) use controller::normalize_scope;

/// TS `isRemoteWorkspaceIdentity`（`packages/shared/src/remote-workspace-identity.ts`）：
/// `remote:<kind>:<authority…>:<posixPath>`，kind ∈ ssh/wsl/docker/server；远端路径必须以 `/` 开头。
/// 远程 source 本片不支持：这类 scope 不回落本地（与 TS 的 `resolveSource` 一致）。
pub(super) fn is_remote_workspace_identity(identity: &str) -> bool {
    let Some(rest) = identity.strip_prefix("remote:") else {
        return false;
    };
    let Some((kind, rest)) = rest.split_once(':') else {
        return false;
    };
    let authority_segments = match kind {
        "ssh" => 3,
        "wsl" | "docker" | "server" => 1,
        _ => return false,
    };
    let mut cursor = rest;
    for _ in 0..authority_segments {
        match cursor.split_once(':') {
            Some((segment, remainder)) if !segment.is_empty() => cursor = remainder,
            _ => return false,
        }
    }
    // WSL 增加了可选 user 段：远端路径必须以 "/" 开头，因此可无歧义地区分。
    if kind == "wsl" && !cursor.starts_with('/') {
        match cursor.split_once(':') {
            Some((segment, remainder)) if !segment.is_empty() => cursor = remainder,
            _ => return false,
        }
    }
    cursor.starts_with('/')
}

/// 组合根：窗口控制器 → 连接级 channel 工厂。
pub(crate) fn controller_channel(controller: Arc<WindowController>) -> ChannelFactory {
    Arc::new(move |_context: &ConnectionContext| connection_channel(&controller))
}

fn connection_channel(controller: &Arc<WindowController>) -> Channel {
    let scope = scope::Scope::new(Arc::clone(controller));
    let subscribe = {
        let scope = Arc::clone(&scope);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let scope = Arc::clone(&scope);
            Box::pin(async move {
                let topic = params["topic"].as_str().unwrap_or_default().to_owned();
                scope.subscribe(&topic).map(Wire::Object)
            })
        })
    };
    let resync = {
        let scope = Arc::clone(&scope);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let scope = Arc::clone(&scope);
            Box::pin(async move {
                let subscription_id = params["subscriptionId"].as_str().unwrap_or_default();
                scope.resync(subscription_id).map(Wire::Object)
            })
        })
    };
    let unsubscribe = {
        let scope = Arc::clone(&scope);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let scope = Arc::clone(&scope);
            Box::pin(async move {
                scope.unsubscribe(params["subscriptionId"].as_str().unwrap_or_default());
                Ok(Wire::Undefined)
            })
        })
    };
    let events: Vec<(&'static str, EventHandler)> = vec![("onDynamicControllerFrame", {
        let scope = Arc::clone(&scope);
        Arc::new(move |_arg: Wire| Ok(scope.listen()))
    })];
    let closing = Arc::clone(&scope);
    let methods: Vec<(&'static str, MethodHandler)> = vec![
        ("subscribeControllerV4", subscribe),
        ("resyncControllerV4", resync),
        ("unsubscribeControllerV4", unsubscribe),
        ("listTaskList", list::list_handler(controller)),
        ("mutateTask", mutate::mutate_handler(controller)),
        (
            "deleteArchivedTask",
            mutate::delete_archived_task_handler(controller),
        ),
        (
            "deleteArchivedTasks",
            mutate::delete_archived_tasks_handler(controller),
        ),
    ];
    Channel::new("window-controller", methods)
        .with_events(events)
        .with_close(Arc::new(move || {
            let scope = Arc::clone(&closing);
            Box::pin(async move { scope.dispose() })
        }))
}

/// 缺 `dataBaseDir` 时的显式缺口表（方法名与有 `dataBaseDir` 时一致，契约对账用；
/// 事件照常可监听但没有帧源——写面全是缺口，不会产生投影变化）。
pub(crate) fn gap_channel() -> Channel {
    let gap = "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.";
    let missing = move || {
        handler_rpc(move |_arg: Wire| {
            let gap = gap.to_owned();
            Box::pin(async move { Err(RpcError::new("NotImplemented", gap)) })
        })
    };
    Channel::new(
        "window-controller",
        vec![
            ("subscribeControllerV4", missing()),
            ("resyncControllerV4", missing()),
            ("unsubscribeControllerV4", missing()),
            ("listTaskList", missing()),
            ("mutateTask", missing()),
            ("deleteArchivedTask", missing()),
            ("deleteArchivedTasks", missing()),
        ],
    )
    .with_events(vec![(
        "onDynamicControllerFrame",
        Arc::new(move |_arg: Wire| {
            Ok(Box::pin(futures_util::stream::empty()) as crate::session::EventStream)
        }),
    )])
}

#[cfg(test)]
mod tests {
    use super::is_remote_workspace_identity;

    /// TS `parseRemoteWorkspaceIdentity` 的判定面：远程 identity 必须命中 kind + authority 段数 +
    /// 以 `/` 开头的远端路径；解析失败一律回落「按本地处理」（不丢 scope，按非远程走本地 source）。
    #[test]
    fn remote_workspace_identity_matches_the_ts_parser() {
        for identity in [
            "remote:ssh:host:22:user:/w",
            "remote:ssh:host:22:user:/",
            "remote:wsl:Ubuntu:/mnt/c/w",
            "remote:wsl:Ubuntu:user:/home/user/w",
            "remote:docker:container:/srv/w",
            "remote:server:host:/srv/w",
        ] {
            assert!(is_remote_workspace_identity(identity), "{identity}");
        }
        for identity in [
            "/local/path",
            "local:/w",
            "remote:",
            "remote:ssh",
            "remote:unknown:host:/w",
            // kind 段数不足：ssh 需要 host/port/username 三段。
            "remote:ssh:host:22:/w",
            // 远端路径必须以 "/" 开头（相对路径整体解析失败）。
            "remote:ssh:host:22:user:relative",
            "remote:docker:container:relative",
            // 空段（`::`）同样不合法。
            "remote:docker::/w",
        ] {
            assert!(!is_remote_workspace_identity(identity), "{identity}");
        }
    }
}
