//! 远端 scope 的事件转发（从 `remote_scope.rs` 拆出以守住 400 行门禁）。
//!
//! 只做流式转发与订阅释放，不解释业务语义。

use std::sync::Arc;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_services::source_server_client::RemoteChannelClient;

use crate::channel::{EventHandler, EventStream};
use crate::wire::Wire;

pub(super) fn event_handler(
    channel: &'static str,
    event: &'static str,
    client: Arc<RemoteChannelClient>,
) -> EventHandler {
    Arc::new(move |arg: Wire| {
        Ok(listener_stream(
            Arc::clone(&client),
            channel,
            event,
            request_arg(arg),
        ))
    })
}

/// `undefined` 实参按「无参」发送（`ChannelClient` 同语义）；其余按值转发。
pub(crate) fn request_arg(arg: Wire) -> Option<WireValue> {
    match arg {
        Wire::Undefined => None,
        other => Some(WireValue::from(other)),
    }
}

/// 远端订阅守卫：流被丢弃（连接关闭 / 取消订阅）时释放远端监听。
struct ListenerGuard {
    client: Arc<RemoteChannelClient>,
    id: u32,
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.client.dispose_listener(self.id);
    }
}

enum ListenerState {
    Pending {
        client: Arc<RemoteChannelClient>,
        channel: &'static str,
        event: &'static str,
        arg: Option<WireValue>,
    },
    Live {
        guard: ListenerGuard,
        receiver: tokio::sync::mpsc::UnboundedReceiver<WireValue>,
    },
}

fn listener_stream(
    client: Arc<RemoteChannelClient>,
    channel: &'static str,
    event: &'static str,
    arg: Option<WireValue>,
) -> EventStream {
    Box::pin(futures_util::stream::unfold(
        ListenerState::Pending {
            client,
            channel,
            event,
            arg,
        },
        |state| async move {
            let mut state = state;
            loop {
                match state {
                    ListenerState::Pending {
                        client,
                        channel,
                        event,
                        arg,
                    } => match client.listen(channel, event, arg).await {
                        Ok((id, receiver)) => {
                            state = ListenerState::Live {
                                guard: ListenerGuard {
                                    client: Arc::clone(&client),
                                    id,
                                },
                                receiver,
                            };
                        }
                        Err(error) => {
                            escode_host_control::logging::warn(
                                "source-server",
                                format!("remote listen {channel}.{event} failed: {error:#}"),
                            );
                            return None;
                        }
                    },
                    ListenerState::Live {
                        guard,
                        mut receiver,
                    } => {
                        return receiver.recv().await.map(|value| {
                            (Wire::from(value), ListenerState::Live { guard, receiver })
                        });
                    }
                }
            }
        },
    ))
}
