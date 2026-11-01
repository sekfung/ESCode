//! 远端 workspace 事件监听（拆出 `remote.rs` 以守住 400 行门禁）。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use escode_host_control::logging;
use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::source_server_client::RemoteChannelClient;

use super::RemoteController;
use super::wire_json;
use crate::wire::Wire;

impl RemoteController {
    pub(crate) fn start_listener(self: &Arc<Self>) {
        let mut listener = self
            .listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if listener.is_some() {
            return;
        }
        let weak = Arc::downgrade(self);
        let client = Arc::clone(&self.client);
        let params = self.scope_params();
        let task = tokio::spawn(async move {
            let arg = Some(WireValue::from(Wire::Object(params)));
            match client
                .listen("escode-task", "onDynamicWorkspaceEvent", arg)
                .await
            {
                Ok((id, mut receiver)) => {
                    let _guard = ListenerGuard {
                        client: Arc::clone(&client),
                        id,
                    };
                    while let Some(value) = receiver.recv().await {
                        let Some(controller) = weak.upgrade() else {
                            return;
                        };
                        controller.publish_unread(&wire_json(Wire::from(value)));
                        controller.dirty.store(true, Ordering::SeqCst);
                        let signal = Arc::clone(&controller);
                        tokio::spawn(async move {
                            let _ = signal.refresh(false).await;
                        });
                    }
                }
                Err(error) => logging::warn(
                    "window-controller",
                    format!("remote workspace event subscription failed: {error:#}"),
                ),
            }
        });
        *listener = Some(task);
    }

    pub(crate) fn stop_listener(&self) {
        if let Some(task) = self
            .listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

struct ListenerGuard {
    client: Arc<RemoteChannelClient>,
    id: u32,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.client.dispose_listener(self.id);
    }
}
