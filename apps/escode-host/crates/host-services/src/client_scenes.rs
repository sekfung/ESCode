//! `client-scenes.list`（docs/specs/rust-u2-client-config.md §4；TS `client-scenes/clientScenesService.ts`）。

use std::sync::Arc;

use serde_json::Value;

use crate::api_client::{ApiClient, ApiError, GetOptions};
use crate::provider::resolve_origin;

/// TS `resolveRuntimeESCodeEndpointOrigin(process.env)`：`ESCODE_ENV` 只有 `test`（不区分大小写）才是测试环境，
/// 其余（含缺省）按生产——和 Main 侧的 `ESCODE_ENV` 判定（只认 `production`）不同，这里照 TS 原样。
/// 与 Endpoint 的设置覆盖无关：TS 在模块加载时算一次，之后由 ApiClient 把默认 Origin 改写到当前 Endpoint。
pub fn resolve_runtime_origin<'a>(get: impl Fn(&str) -> Option<&'a str>) -> String {
    let value = |name: &str| get(name).map(str::trim).filter(|v| !v.is_empty());
    let production = !value("ESCODE_ENV").is_some_and(|env| env.eq_ignore_ascii_case("test"));
    let scoped = value(if production {
        "ESCODE_PRODUCTION_BASE_URL"
    } else {
        "ESCODE_TEST_BASE_URL"
    });
    let base = value("ESCODE_BASE_URL")
        .or_else(|| value("ESCODE_ENDPOINT_ORIGIN"))
        .or(scoped);
    resolve_origin(production, base, None).unwrap_or_else(|_| {
        // TS 在模块加载期抛错会让进程起不来；Host 退回默认 Origin 并让后续请求照常失败/改写。
        crate::provider::DEFAULT_ENDPOINT_ORIGIN.to_owned()
    })
}

pub struct ClientScenes {
    api: Arc<ApiClient>,
    url: String,
}

impl ClientScenes {
    pub fn new(api: Arc<ApiClient>, runtime_origin: &str) -> Self {
        Self {
            api,
            url: format!("{runtime_origin}/api/v1/client/scenes"),
        }
    }

    /// 应答 JSON 原样返回（TS 不校验、不缓存）。
    pub async fn list(&self) -> Result<Value, ApiError> {
        self.api.get_json(&self.url, &GetOptions::default()).await
    }
}

#[cfg(test)]
#[path = "client_scenes_tests.rs"]
mod tests;
