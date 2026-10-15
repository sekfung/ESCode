//! `plugin-management` 的请求整形（docs/specs/rust-u2-plugin-management.md §3；
//! TS `escodeAgentService.ts` 的 `listPlugins` … `setPluginEnabled`，逐条对照）。
//!
//! JSON 语义：TS 的 `undefined` 在序列化时整键消失，所以“缺省”= 键不存在；`null` 会保留。
//! 条件展开分三种：`required` 键存在就带（含 `null`）；`truthy` 键按 JS 真值判定；`defined` 键只要存在就带；
//! `nonempty` 键必须是非空数组（TS 的 `?.length`）。

use std::time::Duration;

use serde_json::{Map, Value};

/// 默认协议超时（TS 走 `ESCodeProtocolClient` 默认值）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// TS `PLUGIN_MANAGEMENT_REQUEST_TIMEOUT_MS`。
const LONG_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// TS `PLUGIN_OPERATION_CANCEL_REQUEST_TIMEOUT_MS`。
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Carrier {
    /// 专用管理面实例（TS `getPluginManagementClient`）。
    Management,
    /// 调用方真实 workspace 的实例（TS `getReadOnlyClient`：持有 session 记录的进程）。
    Workspace,
}

pub(crate) struct Spec {
    pub service: &'static str,
    pub runtime: &'static str,
    pub timeout: Duration,
    pub carrier: Carrier,
    pub with_workspace: bool,
    required: &'static [&'static str],
    truthy: &'static [&'static str],
    defined: &'static [&'static str],
    nonempty: &'static [&'static str],
}

impl Spec {
    /// 带 `workspace` 引用的管理面方法。
    const fn new(service: &'static str, runtime: &'static str, timeout: Duration) -> Self {
        Self {
            service,
            runtime,
            timeout,
            carrier: Carrier::Management,
            with_workspace: true,
            required: &[],
            truthy: &[],
            defined: &[],
            nonempty: &[],
        }
    }

    /// 请求体不带 `workspace`（如 `cancelOperation`）。
    const fn bare(mut self) -> Self {
        self.with_workspace = false;
        self
    }

    const fn required(mut self, keys: &'static [&'static str]) -> Self {
        self.required = keys;
        self
    }

    const fn truthy(mut self, keys: &'static [&'static str]) -> Self {
        self.truthy = keys;
        self
    }

    const fn defined(mut self, keys: &'static [&'static str]) -> Self {
        self.defined = keys;
        self
    }

    const fn nonempty(mut self, keys: &'static [&'static str]) -> Self {
        self.nonempty = keys;
        self
    }
}

const fn long(service: &'static str, runtime: &'static str) -> Spec {
    Spec::new(service, runtime, LONG_TIMEOUT)
}

/// 全部请求/应答方法（`onDynamicPluginOperationProgress` 事件与目录回退单独处理）。
pub(crate) const SPECS: &[Spec] = &[
    Spec::new("listPlugins", "plugins/list", DEFAULT_TIMEOUT).truthy(&["configScope"]),
    long("getPluginsOverview", "plugins/overview").truthy(&["configScope"]),
    long("addPluginMarketplace", "plugins/marketplace/add")
        .required(&["source"])
        .truthy(&["operationId"])
        .defined(&["dryRun"]),
    long("removePluginMarketplace", "plugins/marketplace/remove").required(&["marketplace"]),
    long("updatePluginMarketplace", "plugins/marketplace/update")
        .truthy(&["marketplace", "operationId"]),
    long("installPlugin", "plugins/install")
        .required(&["pluginName", "marketplace"])
        .truthy(&["scope", "operationId"])
        .defined(&["dryRun"]),
    Spec::new(
        "cancelPluginOperation",
        "plugins/cancelOperation",
        CANCEL_TIMEOUT,
    )
    .bare()
    .required(&["operationId"]),
    long("uninstallPlugin", "plugins/uninstall")
        .truthy(&["pluginId", "pluginName", "marketplace"])
        .defined(&["removeCache"]),
    long("updatePlugin", "plugins/update").truthy(&["pluginId", "marketplace"]),
    long("restoreBuiltinPlugin", "plugins/restoreBuiltin").required(&["pluginId"]),
    long("configurePlugin", "plugins/configure")
        .required(&["pluginId", "options"])
        .truthy(&["scope"])
        .defined(&["dryRun"])
        .nonempty(&["clearOptionKeys"]),
    long("resetPluginConfig", "plugins/resetConfig")
        .required(&["pluginId"])
        .truthy(&["scope"]),
    long("validatePlugin", "plugins/validate").truthy(&["pluginName", "marketplace", "source"]),
    long("describePlugin", "plugins/describe").required(&["marketplace", "pluginName"]),
    long("setPluginEnabled", "plugins/setEnabled")
        .required(&["pluginId", "enabled"])
        .truthy(&["operationId", "scope"]),
    long(
        "resolveSuggestedPluginReference",
        "plugins/resolveSuggestedReference",
    )
    .required(&["stableId", "operationId", "clientMode", "deliveryKind"]),
];

/// TS `getPluginReferenceCatalog` 的两个 runtime 方法（新名先试，仅 `-32601` 回退）。
pub(crate) const CATALOG_METHODS: (&str, &str) = (
    "plugins/referenceCatalogWithCategory",
    "plugins/referenceCatalog",
);
pub(crate) const CATALOG_TIMEOUT: Duration = DEFAULT_TIMEOUT;

/// JS 真值（`x ? … : …`）。
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

impl Spec {
    /// 请求体：`workspace`（若该方法带）+ 各类键的条件展开。
    pub(crate) fn body(&self, params: &Value, workspace: &Value) -> Value {
        let mut body = Map::new();
        if self.with_workspace {
            body.insert("workspace".to_owned(), workspace.clone());
        }
        let get = |key: &str| params.get(key);
        for key in self.required {
            if let Some(value) = get(key) {
                body.insert((*key).to_owned(), value.clone());
            }
        }
        for key in self.truthy {
            if let Some(value) = get(key).filter(|value| truthy(value)) {
                body.insert((*key).to_owned(), value.clone());
            }
        }
        for key in self.defined {
            if let Some(value) = get(key) {
                body.insert((*key).to_owned(), value.clone());
            }
        }
        for key in self.nonempty {
            if let Some(value) = get(key).filter(|v| v.as_array().is_some_and(|a| !a.is_empty())) {
                body.insert((*key).to_owned(), value.clone());
            }
        }
        Value::Object(body)
    }
}

/// `getPluginReferenceCatalog` 的请求体：`workspace` + `sessionId?`(真值)。
pub(crate) fn catalog_body(params: &Value, workspace: &Value) -> Value {
    let mut body = Map::new();
    body.insert("workspace".to_owned(), workspace.clone());
    if let Some(session_id) = params.get("sessionId").filter(|value| truthy(value)) {
        body.insert("sessionId".to_owned(), session_id.clone());
    }
    Value::Object(body)
}

#[cfg(test)]
#[path = "plugin_params_tests.rs"]
mod tests;
