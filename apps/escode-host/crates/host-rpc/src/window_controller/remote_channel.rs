//! 远端 scope 的 `window-controller` channel 装配（行为见 `remote.rs`）。

use std::sync::Arc;

use escode_host_control::logging;
use escode_host_services::source_server_client::RemoteChannelClient;

use super::remote::RemoteController;
use super::scope::Scope;
use crate::domains_task_support::first_object;
use crate::remote_scope::RemoteScope;
use crate::session::{Channel, EventHandler, MethodHandler, handler_rpc};
use crate::wire::Wire;

/// 每条远端 scope 连接一份 controller、投影与订阅表。
pub(crate) fn remote_channel(client: Arc<RemoteChannelClient>, scope: &RemoteScope) -> Channel {
    let controller = RemoteController::new(client, scope);
    let subscription = Scope::new(Arc::clone(controller.projector()));

    let subscribe = {
        let controller = Arc::clone(&controller);
        let subscription = Arc::clone(&subscription);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let controller = Arc::clone(&controller);
            let subscription = Arc::clone(&subscription);
            Box::pin(async move {
                if let Err(error) = controller.refresh(false).await {
                    logging::warn(
                        "window-controller",
                        format!("remote snapshot refresh failed: {}", error.message),
                    );
                }
                let topic = params["topic"].as_str().unwrap_or_default().to_owned();
                subscription.subscribe(&topic).map(Wire::Object)
            })
        })
    };
    let resync = {
        let subscription = Arc::clone(&subscription);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let subscription = Arc::clone(&subscription);
            Box::pin(async move {
                subscription
                    .resync(params["subscriptionId"].as_str().unwrap_or_default())
                    .map(Wire::Object)
            })
        })
    };
    let unsubscribe = {
        let subscription = Arc::clone(&subscription);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let subscription = Arc::clone(&subscription);
            Box::pin(async move {
                subscription.unsubscribe(params["subscriptionId"].as_str().unwrap_or_default());
                Ok(Wire::Undefined)
            })
        })
    };
    let frames: EventHandler = {
        let subscription = Arc::clone(&subscription);
        Arc::new(move |_arg: Wire| Ok(subscription.listen()))
    };
    let list = {
        let controller = Arc::clone(&controller);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let controller = Arc::clone(&controller);
            Box::pin(async move { controller.list_task_list(params).await.map(Wire::Object) })
        })
    };
    let mutate = {
        let controller = Arc::clone(&controller);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let controller = Arc::clone(&controller);
            Box::pin(async move {
                controller
                    .mutate_task(&params["address"], &params["mutation"])
                    .await
                    .map(Wire::Object)
            })
        })
    };
    let delete_one = {
        let controller = Arc::clone(&controller);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let controller = Arc::clone(&controller);
            Box::pin(async move {
                controller
                    .delete_archived_task(&params["address"])
                    .await
                    .map(Wire::Object)
            })
        })
    };
    let delete_many = {
        let controller = Arc::clone(&controller);
        handler_rpc(move |arg: Wire| {
            let params = first_object(arg);
            let controller = Arc::clone(&controller);
            Box::pin(async move {
                controller
                    .delete_archived_tasks(&params)
                    .await
                    .map(Wire::Object)
            })
        })
    };
    let methods: Vec<(&'static str, MethodHandler)> = vec![
        ("subscribeControllerV4", subscribe),
        ("resyncControllerV4", resync),
        ("unsubscribeControllerV4", unsubscribe),
        ("listTaskList", list),
        ("mutateTask", mutate),
        ("deleteArchivedTask", delete_one),
        ("deleteArchivedTasks", delete_many),
    ];

    let open = {
        let controller = Arc::clone(&controller);
        Arc::new(move || controller.start_listener())
    };
    let close = {
        let controller = Arc::clone(&controller);
        Arc::new(move || {
            controller.stop_listener();
            Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        })
    };
    Channel::new("window-controller", methods)
        .with_events(vec![
            ("onDynamicControllerFrame", frames),
            ("onDynamicTaskUnreadSignal", {
                let unread = Arc::clone(controller.unread());
                Arc::new(move |_arg: Wire| Ok(unread.listen()))
            }),
        ])
        .with_open(open)
        .with_close(close)
}
