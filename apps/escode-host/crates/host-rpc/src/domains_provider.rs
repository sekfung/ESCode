//! providers 域的 RPC 面（docs/specs/rust-u2-providers.md）。P1：`model-selection.getView`；P2：`provider-settings.getView/refresh`。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::provider::ProviderService;

use crate::session::{Channel, MethodHandler, handler};
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
        ),
        Channel::new("provider-settings", {
            let mut methods = vec![
                ("getView", settings_view(service.clone())),
                ("refresh", settings_view(service.clone())),
            ];
            methods.extend(crate::domains_provider_write::write_methods(&service));
            methods
        }),
    ]
}
