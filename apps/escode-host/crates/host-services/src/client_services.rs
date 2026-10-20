//! `client-config` / `client-scenes` 的装配（docs/specs/rust-u2-client-config.md）：一个 ApiClient，多个服务共用。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api_client::{ApiClient, ClientIdentity, NetTransport, Transport};
use crate::client_config::ClientConfigService;
use crate::client_scenes::{ClientScenes, resolve_runtime_origin};
use crate::provider::EndpointSource;

pub struct ClientServices {
    pub api: Arc<ApiClient>,
    pub scenes: ClientScenes,
    pub config: ClientConfigService,
}

impl ClientServices {
    /// `get` 读 `rpcListen.runtimeEnv`；`home` = 设置根（Endpoint 覆盖值所在）。
    pub fn from_env<'a>(
        get: impl Fn(&str) -> Option<&'a str> + Copy,
        home: Option<&Path>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        let value = |name: &str| get(name).map(str::trim).filter(|v| !v.is_empty());
        let endpoint = EndpointSource {
            escode_env: value("ESCODE_ENV").map(str::to_owned),
            base_origin: value("ESCODE_ENDPOINT_ORIGIN").map(str::to_owned),
            settings_home: home.map(Path::to_path_buf),
        };
        let identity =
            ClientIdentity::from_env(get, value("ESCODE_BUILTIN_CACHE_ROOT").map(PathBuf::from));
        let app_version = value("ESCODE_APP_VERSION").unwrap_or("unknown").to_owned();
        let api = Arc::new(ApiClient::new(endpoint, identity, transport));
        let scenes = ClientScenes::new(Arc::clone(&api), &resolve_runtime_origin(get));
        let config = ClientConfigService::new(
            Arc::clone(&api),
            app_version,
            ClientIdentity::platform_arch(),
        )
        .with_dynamic_workflow(
            // TS 读进程 env `ESCODE_DYNAMIC_WORKFLOW_MODE`（Main 按构建档位改写/删除后下发）。
            get("ESCODE_DYNAMIC_WORKFLOW_MODE").map(str::to_owned),
            home.map(Path::to_path_buf),
        );
        Self {
            api,
            scenes,
            config,
        }
    }

    /// 生产装配：统一出口。
    pub fn production<'a>(
        get: impl Fn(&str) -> Option<&'a str> + Copy,
        home: Option<&Path>,
    ) -> Self {
        Self::from_env(get, home, Arc::new(NetTransport))
    }
}
