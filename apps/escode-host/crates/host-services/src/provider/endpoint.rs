//! 控制面 Endpoint 的解析（TS `shared/escodeEndpoint.ts: resolveESCodeEndpointOrigin`，spec §12）。
//!
//! 设置项 `escodeEndpointOrigin` 在 Host 自己持有的设置文件里，所以切换 Endpoint 不需要控制消息：
//! 每次读取重新解析，路径/指纹随之变化。只读，不隔离坏文件（settings 域才是设置文件的写者）。

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::builtin_source::normalize_endpoint_origin;

/// TS `DEFAULT_ESCODE_ENDPOINT_ORIGIN` / `TEST_ESCODE_ENDPOINT_ORIGIN`（当前两者相同）。
pub const DEFAULT_ORIGIN: &str = "https://zcode.z.ai";

#[derive(Debug, Clone, Default)]
pub struct EndpointSource {
    /// `ESCODE_ENV`：仅 `production` 为生产（忽略设置覆盖）；缺省按 `test`（TS 默认值）。
    pub escode_env: Option<String>,
    /// env 基址（TS `envBaseOrigin`）；可缺省。
    pub base_origin: Option<String>,
    /// 设置根目录（读 `escodeEndpointOrigin` 覆盖）；缺省 = 无覆盖来源。
    pub settings_home: Option<PathBuf>,
}

impl EndpointSource {
    pub fn settings_file(&self) -> Option<PathBuf> {
        self.settings_home
            .as_deref()
            .map(crate::settings::settings_path)
    }

    /// 规范化后的当前 Endpoint Origin。
    pub async fn resolve(&self) -> Result<String> {
        let production = self.escode_env.as_deref().map(str::trim) == Some("production");
        let override_origin = match (&self.settings_home, production) {
            (Some(home), false) => read_override(home).await,
            _ => None,
        };
        resolve_origin(
            production,
            self.base_origin.as_deref(),
            override_origin.as_deref(),
        )
    }
}

/// TS `resolveESCodeEndpointOrigin` 的纯函数部分。
pub fn resolve_origin(
    production: bool,
    base: Option<&str>,
    override_origin: Option<&str>,
) -> Result<String> {
    let base = base.map(str::trim).filter(|value| !value.is_empty());
    if production {
        return base.map_or_else(|| Ok(DEFAULT_ORIGIN.to_owned()), normalize_endpoint_origin);
    }
    let override_origin = override_origin
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(origin) = override_origin {
        return normalize_endpoint_origin(origin);
    }
    base.map_or_else(|| Ok(DEFAULT_ORIGIN.to_owned()), normalize_endpoint_origin)
}

async fn read_override(home: &Path) -> Option<String> {
    let text = tokio::fs::read_to_string(crate::settings::settings_path(home))
        .await
        .ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value
        .get("escodeEndpointOrigin")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "endpoint_tests.rs"]
mod tests;
