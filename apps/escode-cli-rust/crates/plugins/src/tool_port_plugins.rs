//! ToolPort 的插件面投影：参数校验与实现都交给插件模块（docs/specs/rust-plugins.md）。
use anyhow::Result;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// `plugins/list`（docs/specs/rust-plugins.md 第 1 期）：workspace 级、无会话。
pub async fn list(params: &Value) -> Result<Value> {
    let cwd = super::plugin_list::workspace_path(params)?;
    let layers = super::plugin_list::layers_for(&cwd, params).await?;
    super::plugin_list::list(&cwd, &layers, &CancellationToken::new()).await
}

/// `plugins/listUiSurfaces`：已启用插件的 `ui.surfaces[]` 只读投影。
pub async fn ui_surfaces(params: &Value) -> Result<Value> {
    super::plugin_ui_surfaces::list(params).await
}

/// `plugins/setEnabled`（docs/specs/rust-plugins.md 第 2 期）。
pub async fn set_enabled(params: &Value) -> Result<Value> {
    super::plugin_list::set_enabled(params, &CancellationToken::new()).await
}

/// `plugins/configure` / `plugins/resetConfig`（docs/specs/rust-plugins.md 第 4 期选项面）。
pub async fn configure(params: &Value, raw_params: Option<&str>) -> Result<Value> {
    super::plugin_config::configure(params, raw_params, &CancellationToken::new()).await
}

pub async fn reset_config(params: &Value) -> Result<Value> {
    super::plugin_config::reset(params).await
}

/// 插件写面的慢操作（docs/specs/rust-plugin-marketplace-write.md W1b–W5）。
pub async fn operation(method: &str, params: &Value, cancel: &CancellationToken) -> Result<Value> {
    match method {
        "plugins/install" => super::plugin_install::install(params, cancel).await,
        "plugins/update" => super::plugin_install::update(params, cancel).await,
        "plugins/describe" => super::plugin_describe::describe(params).await,
        "plugins/validate" => super::plugin_validate::validate(params).await,
        "plugins/marketplace/add" => super::plugin_market_write::add_params(params, cancel).await,
        "plugins/marketplace/remove" => super::plugin_market_write::remove_params(params).await,
        "plugins/marketplace/update" => {
            super::plugin_market_write::update_params(params, cancel).await
        }
        _ => anyhow::bail!("Unsupported plugin operation: {method}"),
    }
}

pub async fn suggested_reference(
    params: &Value,
    refresh: bool,
    cancel: &CancellationToken,
) -> Result<Option<Value>> {
    super::plugin_suggested::resolve(params, refresh, cancel).await
}

/// `plugins/uninstall` / `plugins/restoreBuiltin`（docs/specs/rust-plugin-marketplace-write.md W1a）。
pub async fn uninstall(params: &Value) -> Result<Value> {
    super::plugin_uninstall::uninstall(params, &CancellationToken::new()).await
}

pub async fn restore_builtin(params: &Value) -> Result<Value> {
    super::plugin_uninstall::restore_builtin(params).await
}

/// `plugins/overview`（docs/specs/rust-plugins.md 第 3 期）。
pub async fn overview(params: &Value) -> Result<Value> {
    super::plugin_overview::overview(params, &CancellationToken::new()).await
}

/// `plugins/referenceCatalog(WithCategory)`（docs/specs/rust-plugins.md 第 3 期）。
pub async fn reference_catalog(
    params: &Value,
    frozen: Option<&Value>,
    include_category: bool,
) -> Result<Value> {
    let cancel = CancellationToken::new();
    super::plugin_reference::catalog(params, frozen, include_category, &cancel).await
}
