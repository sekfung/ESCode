//! CUA 权限观察事件（`v4/cua/permission-observation`）的 Host 侧扇出。
//!
//! TS `escodeAgentService` 从 runtime 通知流里转发该事件（只限本地 workspace）。Host 的
//! `Instances` 是唯一实例表，这里在实例首次被使用时订阅其出站帧，并按 workspace 过滤。

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use escode_host_runtime::Instance;
use futures_util::stream;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::channel::EventStream;
use crate::wire::Wire;

const METHOD: &str = "v4/cua/permission-observation";

pub(crate) struct CuaObservations {
    sender: broadcast::Sender<Value>,
    pumped: Mutex<BTreeSet<(String, u64)>>,
}

impl Default for CuaObservations {
    fn default() -> Self {
        Self {
            sender: broadcast::channel(256).0,
            pumped: Mutex::default(),
        }
    }
}

impl CuaObservations {
    pub(crate) async fn attach(&self, instance: &Arc<Instance>) {
        let key = (instance.identity.clone(), instance.generation);
        {
            let mut pumped = self
                .pumped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !pumped.insert(key) {
                return;
            }
        }
        let mut frames = instance.subscribe().await;
        let sender = self.sender.clone();
        let path = instance.workspace_path().to_owned();
        let identity = instance.workspace_identity().map(str::to_owned);
        // TS 只转发本地 workspace 的权限观察；远程 workspace 由远端 Host 自己上报。
        if identity
            .as_deref()
            .is_some_and(crate::window_controller::is_remote_workspace_identity)
        {
            return;
        }
        tokio::spawn(async move {
            while let Some(line) = frames.recv().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message["method"] != METHOD {
                    continue;
                }
                let mut observation = message["params"].clone();
                if !observation.is_object() {
                    continue;
                }
                observation["workspacePath"] = Value::from(path.clone());
                if let Some(identity) = &identity {
                    observation["workspaceIdentity"] = Value::from(identity.clone());
                }
                // 无订阅者时忽略；观察事件不落盘、不补发。
                let _ = sender.send(observation);
            }
        });
    }

    pub(crate) fn event(&self, workspace: Option<String>) -> EventStream {
        let receiver = self.sender.subscribe();
        Box::pin(stream::unfold(
            (receiver, workspace),
            |(mut receiver, workspace)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(value) => {
                            let matches = workspace.as_deref().is_none_or(|target| {
                                value["workspaceKey"].as_str() == Some(target)
                                    || value["workspacePath"].as_str() == Some(target)
                                    || value["workspaceIdentity"].as_str() == Some(target)
                            });
                            if matches {
                                return Some((Wire::Object(value), (receiver, workspace)));
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        ))
    }
}
