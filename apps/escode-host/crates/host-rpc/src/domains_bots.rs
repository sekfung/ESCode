//! `bots` channel 的配置/状态/绑定子集（docs/specs/rust-u2-bots.md）。
//!
//! 平台 provider、入站消息、群投递、附件与 V4 流仍未迁移；这些成员不在方法表里，
//! Gateway 回显式 `NotImplemented`。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::bots::BotsService;
use serde_json::Value;

use crate::session::{Channel, MethodHandler, handler};
use crate::wire::Wire;

#[allow(clippy::type_complexity)]
type BotFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send>>;
#[allow(clippy::type_complexity)]
type BotUnit = fn(Arc<BotsService>) -> BotFuture;
#[allow(clippy::type_complexity)]
type BotId = fn(Arc<BotsService>, String) -> BotFuture;
#[allow(clippy::type_complexity)]
type BotObject = fn(Arc<BotsService>, Value) -> BotFuture;

fn args(arg: &Wire) -> Vec<Wire> {
    match arg {
        Wire::Array(items) => items.clone(),
        Wire::Undefined => Vec::new(),
        other => vec![other.clone()],
    }
}

fn object_arg(arg: &Wire) -> Value {
    args(arg)
        .into_iter()
        .next()
        .and_then(Wire::into_object)
        .unwrap_or(Value::Null)
}

fn string_arg(arg: &Wire) -> Result<String, (String, String)> {
    args(arg)
        .first()
        .and_then(Wire::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            (
                "InvalidRequest".to_owned(),
                "bots method requires an id".to_owned(),
            )
        })
}

fn gap() -> MethodHandler {
    handler(|_arg: Wire| {
        Box::pin(async move {
            Err((
                "NotImplemented".to_owned(),
                "bots needs dataBaseDir: Main must pass dataBaseDir in rpcListen.".to_owned(),
            ))
        })
    })
}

pub(crate) fn bots_channel(data_base_dir: Option<PathBuf>, home: PathBuf) -> Channel {
    let service = data_base_dir.map(|dir| Arc::new(BotsService::new(dir, home)));
    let unit = |run: BotUnit| -> MethodHandler {
        match service.clone() {
            Some(service) => handler(move |_arg: Wire| {
                let service = Arc::clone(&service);
                Box::pin(async move {
                    run(service)
                        .await
                        .map(Wire::Object)
                        .map_err(|message| ("Error".to_owned(), message))
                })
            }),
            None => gap(),
        }
    };
    let by_id = |run: BotId| -> MethodHandler {
        match service.clone() {
            Some(service) => handler(move |arg: Wire| {
                let (service, id) = (Arc::clone(&service), string_arg(&arg));
                Box::pin(async move {
                    run(service, id?)
                        .await
                        .map(Wire::Object)
                        .map_err(|message| ("Error".to_owned(), message))
                })
            }),
            None => gap(),
        }
    };
    let by_object = |run: BotObject| -> MethodHandler {
        match service.clone() {
            Some(service) => handler(move |arg: Wire| {
                let (service, params) = (Arc::clone(&service), object_arg(&arg));
                Box::pin(async move {
                    run(service, params)
                        .await
                        .map(Wire::Object)
                        .map_err(|message| ("Error".to_owned(), message))
                })
            }),
            None => gap(),
        }
    };
    let delete_bot = match service.clone() {
        Some(service) => handler(move |arg: Wire| {
            let (service, id) = (Arc::clone(&service), string_arg(&arg));
            Box::pin(async move {
                service
                    .delete_bot(id?)
                    .await
                    .map(|_| Wire::Undefined)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        }),
        None => gap(),
    };
    let sync_preferences = match service.clone() {
        Some(service) => handler(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .sync_app_runtime_preferences()
                    .await
                    .map(|_| Wire::Undefined)
                    .map_err(|message| ("Error".to_owned(), message))
            })
        }),
        None => gap(),
    };

    Channel::new(
        "bots",
        vec![
            (
                "getConfig",
                unit(|s| Box::pin(async move { s.get_config().await })),
            ),
            (
                "getStatus",
                unit(|s| Box::pin(async move { s.get_status().await })),
            ),
            (
                "listBots",
                unit(|s| Box::pin(async move { s.list_bots().await })),
            ),
            (
                "getBotStates",
                unit(|s| Box::pin(async move { s.get_bot_states().await })),
            ),
            (
                "saveConfig",
                by_object(|s, p| Box::pin(async move { s.save_config(p).await })),
            ),
            (
                "saveBot",
                by_object(|s, p| Box::pin(async move { s.save_bot(p).await })),
            ),
            (
                "removeBotSecret",
                by_id(|s, id| Box::pin(async move { s.remove_bot_secret(id).await })),
            ),
            (
                "resetBotState",
                by_id(|s, id| Box::pin(async move { s.reset_bot_state(id).await })),
            ),
            (
                "createBindCode",
                by_object(|s, p| Box::pin(async move { s.create_bind_code(p).await })),
            ),
            (
                "listWorkspaceRefs",
                by_object(|s, p| Box::pin(async move { s.list_workspace_refs(p).await })),
            ),
            ("deleteBot", delete_bot),
            ("syncAppRuntimePreferences", sync_preferences),
        ],
    )
}
