//! providers 域的 RPC 面（docs/specs/rust-u2-providers.md）。P1：`model-selection.getView`；P2：`provider-settings.getView/refresh`。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::provider::ProviderService;

use futures_util::stream;
use serde_json::Value;

use crate::session::{Channel, EventHandler, EventStream, MethodHandler, handler};
use crate::wire::Wire;

/// 缺 provider 配置文件路径（Main 未经 `rpcListen.runtimeEnv` 提供）：显式缺口，不按 HOME 猜。
pub(crate) fn missing_files() -> (String, String) {
    (
        "InvalidRequest".to_owned(),
        "Host has no provider config files: Main must pass ESCODE_BUILTIN_PROVIDER_CONFIG_FILE and ESCODE_PERSONAL_PROVIDER_CONFIG_FILE in rpcListen.runtimeEnv."
            .to_owned(),
    )
}

type Service = Option<Arc<ProviderService>>;

pub(crate) fn error(error: anyhow::Error) -> (String, String) {
    ("Error".to_owned(), format!("{error:#}"))
}

fn model_selection_get_view(service: Service) -> MethodHandler {
    handler(move |arg: Wire| {
        let service = service.clone();
        Box::pin(async move {
            let Some(service) = service else {
                return Err(missing_files());
            };
            // `getView(input?)`：`ProxyChannel.toService` 把实参打包成数组，缺省为空数组。
            let input = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object);
            service
                .model_selection_view(input.as_ref())
                .await
                .map(Wire::Object)
                .map_err(error)
        })
    })
}

fn settings_view(service: Service) -> MethodHandler {
    handler(move |_arg: Wire| {
        let service = service.clone();
        Box::pin(async move {
            let Some(service) = service else {
                return Err(missing_files());
            };
            service
                .provider_settings_view()
                .await
                .map(Wire::Object)
                .map_err(error)
        })
    })
}

/// providers 域的全部 channel（共享同一个 `ProviderService`）。
pub fn provider_channels(files: Option<(PathBuf, PathBuf)>) -> Vec<Channel> {
    let service: Service =
        files.map(|(builtin, personal)| Arc::new(ProviderService::new(builtin, personal)));
    vec![
        Channel::new(
            "model-selection",
            vec![("getView", model_selection_get_view(service.clone()))],
        )
        .with_events(vec![(
            "onDidChange",
            change_event(service.clone(), |service| async move {
                service.model_selection_view(None).await
            }),
        )]),
        Channel::new("provider-settings", {
            let mut methods = vec![
                ("getView", settings_view(service.clone())),
                ("refresh", settings_view(service.clone())),
            ];
            methods.extend(crate::domains_provider_write::write_methods(&service));
            methods
        })
        .with_events(vec![(
            "onDidChange",
            change_event(service, |service| async move {
                service.provider_settings_view().await
            }),
        )]),
    ]
}

/// 变更流的状态：首次轮询时才订阅（`watch_changes` 是异步的，而事件工厂是同步的）。
enum Feed {
    Start(Arc<ProviderService>),
    Running(Arc<ProviderService>, tokio::sync::watch::Receiver<u64>),
}

/// `onDidChange`：每次 Registry 内容变化推一份**新算出的**视图（TS 同样在事件时重建 View）。
fn change_event<F, Fut>(service: Service, render: F) -> EventHandler
where
    F: Fn(Arc<ProviderService>) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = anyhow::Result<Value>> + Send + 'static,
{
    Arc::new(move |_arg: Wire| {
        let Some(service) = service.clone() else {
            return Err(missing_files().1);
        };
        let render = render.clone();
        let stream: EventStream = Box::pin(stream::unfold(Feed::Start(service), move |feed| {
            let render = render.clone();
            async move {
                let (service, mut receiver) = match feed {
                    Feed::Start(service) => {
                        let receiver = service.watch_changes().await;
                        (service, receiver)
                    }
                    Feed::Running(service, receiver) => (service, receiver),
                };
                loop {
                    receiver.changed().await.ok()?;
                    match render(Arc::clone(&service)).await {
                        Ok(view) => {
                            return Some((Wire::Object(view), Feed::Running(service, receiver)));
                        }
                        // 视图重建失败只丢这一次通知（TS：warn）；下次变化再推。
                        Err(error) => {
                            eprintln!("escode-host: provider view refresh failed: {error:#}")
                        }
                    }
                }
            }
        }));
        Ok(stream)
    })
}
