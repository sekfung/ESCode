//! `plugins` 通道（TS `createPluginsService`，`packages/services/src/plugins/pluginsService.ts`）。
//!
//! **整套旧插件管理已退役**：`getOverview` 回固定的「无能力」空概览，其余 6 个方法一律抛
//! `Legacy plugin management has been retired in ESCode Agent mode`。因此本域是纯常量应答——
//! **不解析参数**（TS 形参前缀 `_params`，连校验都没有）、不接运行时、不触文件。
//! 活的插件管理是 `plugin-management` 通道（`domains_plugin.rs`）。
//!
//! 特别注意与惯例相反的一条：缺 `workspacePath` 也**不能**回 `InvalidRequest`——TS 忽略参数，
//! 必须照常回常量 / 抛退役错。见 `rust-u2-plugin-management.md` §8。

use crate::channel::Channel;
use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

/// TS `throwRetiredPluginManagement`：`Error`（无 `code`），文案逐字对齐。
const RETIRED_MESSAGE: &str = "Legacy plugin management has been retired in ESCode Agent mode";

fn retired() -> (String, String) {
    ("Error".to_owned(), RETIRED_MESSAGE.to_owned())
}

/// 退役的 6 个写方法：一律抛同一错误（TS 对每个方法各写一遍 `throwRetiredPluginManagement()`）。
fn retired_methods() -> Vec<(&'static str, MethodHandler)> {
    [
        "addMarketplace",
        "removeMarketplace",
        "updateMarketplace",
        "installPlugin",
        "uninstallPlugin",
        "setPluginEnabled",
    ]
    .into_iter()
    .map(|name| {
        let method: MethodHandler = handler(|_arg: Wire| Box::pin(async move { Err(retired()) }));
        (name, method)
    })
    .collect()
}

pub(crate) fn plugins_channel() -> Channel {
    let mut methods = vec![(
        "getOverview",
        // TS `createRetiredOverview`：四个字段的固定值，`capability.supported` 恒为 false。
        handler(|_arg: Wire| {
            Box::pin(async move {
                Ok(Wire::Object(serde_json::json!({
                    "marketplaces": [],
                    "availablePlugins": [],
                    "installedPlugins": [],
                    "capability": {"supported": false},
                })))
            })
        }) as MethodHandler,
    )];
    methods.extend(retired_methods());
    Channel::new("plugins", methods)
}

#[cfg(test)]
#[path = "domains_plugins_legacy_tests.rs"]
mod tests;
