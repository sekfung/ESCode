//! `plugins/resolveSuggestedReference`（docs/specs/rust-plugin-marketplace-write.md W5b）：对齐 TS
//! `escode-protocol/plugin-reference-catalog.ts::resolveSuggestedPluginReference`。推荐 Prompt 安装前的可信解析：
//! 只接受官方市场的 stable id；本地引用目录命中即返回 ready / disabled / conflict，否则刷新官方目录（10 s 超时，
//! 失败禁止用旧快照安装）后再查，仍未安装则按刷新后的目录返回 missing。
//!
//! 两段式：`refresh = false` 只做本地判定（未命中返回 None，engine 随即发 `plugins/operationProgress`
//! refreshing 通知）；`refresh = true` 刷新后给出最终结果。

use super::plugin_marketplace::OFFICIAL_MARKETPLACE;
use super::{plugin_market_write as market_write, plugin_overview, plugin_reference as reference};
use anyhow::Result;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const REFRESH_TIMEOUT: Duration = Duration::from_millis(10_000);
static STABLE_ID: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*@[A-Za-z0-9][A-Za-z0-9._-]*$")
        .expect("stable id pattern")
});

pub(super) async fn resolve(
    params: &Value,
    refresh: bool,
    cancel: &CancellationToken,
) -> Result<Option<Value>> {
    let stable_id = super::plugin_list::non_empty(params, "stableId")?.to_owned();
    super::plugin_list::non_empty(params, "operationId")?;
    let (plugin_name, marketplace) = match stable_id.rfind('@') {
        Some(at) if at > 0 => (&stable_id[..at], &stable_id[at + 1..]),
        _ => ("", ""),
    };
    let unavailable = |code: &str, message: &str| {
        Some(json!({
            "stableId": stable_id,
            "status": "unavailable",
            "diagnostics": [diagnostic(&stable_id, code, message)],
        }))
    };
    if plugin_name.is_empty()
        || marketplace != OFFICIAL_MARKETPLACE
        || !STABLE_ID.is_match(&stable_id)
    {
        return Ok(unavailable(
            "plugin_suggested_reference_untrusted_source",
            "推荐插件不是受信任的官方 escode-plugins-official 来源",
        ));
    }
    let workspace = json!({ "workspace": params["workspace"] });
    if let Some(result) = local(&workspace, &stable_id, plugin_name, cancel).await? {
        return Ok(Some(result));
    }
    if !refresh {
        return Ok(None);
    }

    // 超时必须中止底层刷新（丢弃 future 即中止网络 / 子进程等待），不能让旧操作继续改写目录快照。
    let refresh_cancel = cancel.child_token();
    let request = json!({ "workspace": params["workspace"], "marketplace": OFFICIAL_MARKETPLACE });
    let refreshed = tokio::time::timeout(
        REFRESH_TIMEOUT,
        market_write::update_params(&request, &refresh_cancel),
    )
    .await;
    let cancelled = || unavailable("plugin_operation_cancelled", "插件操作已取消");
    match refreshed {
        Err(_) => {
            refresh_cancel.cancel();
            return Ok(unavailable(
                "marketplace_refresh_failed",
                "刷新 escode-plugins-official 超时（10000 ms）",
            ));
        }
        Ok(Err(_)) if cancel.is_cancelled() => return Ok(cancelled()),
        Ok(Err(error)) => {
            return Ok(unavailable(
                "marketplace_refresh_failed",
                &error.to_string(),
            ));
        }
        Ok(Ok(_)) if cancel.is_cancelled() => return Ok(cancelled()),
        Ok(Ok(result)) => {
            let failure = result["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|d| d["pluginId"] == OFFICIAL_MARKETPLACE);
            if let Some(failure) = failure {
                let message = failure["message"].as_str().unwrap_or_default();
                return Ok(unavailable("marketplace_refresh_failed", message));
            }
        }
    }

    if let Some(result) = local(&workspace, &stable_id, plugin_name, cancel).await? {
        return Ok(Some(result));
    }
    let overview = plugin_overview::overview(&workspace, cancel).await?;
    let candidate = overview["availablePlugins"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["id"] == stable_id.as_str());
    let Some(candidate) =
        candidate.filter(|c| c["name"] == plugin_name && c["marketplace"] == OFFICIAL_MARKETPLACE)
    else {
        return Ok(unavailable(
            "plugin_suggested_reference_not_listed",
            "刷新后的官方目录中未找到该插件",
        ));
    };
    let mut out = json!({
        "stableId": stable_id,
        "status": "missing",
        "marketplace": OFFICIAL_MARKETPLACE,
        "pluginName": candidate["name"],
        "sourceTrust": "official",
    });
    if let Some(icon) = candidate["listing"]["icon"]
        .as_str()
        .map(str::trim)
        .filter(|i| !i.is_empty())
    {
        out["icon"] = icon.into();
    }
    if candidate["listing"].is_object() {
        out["listing"] = candidate["listing"].clone();
    }
    out["diagnostics"] = json!([]);
    Ok(Some(out))
}

fn diagnostic(stable_id: &str, code: &str, message: &str) -> Value {
    json!({ "code": code, "message": message, "severity": "error", "pluginId": stable_id })
}

/// 本地引用目录（全部已发现插件）里命中 stable id 时的结果；图标取已缓存的官方 listing。
async fn local(
    workspace: &Value,
    stable_id: &str,
    plugin_name: &str,
    cancel: &CancellationToken,
) -> Result<Option<Value>> {
    let cwd = super::plugin_list::workspace_path(workspace)?;
    let entries = reference::identity_entries(&cwd, cancel).await?;
    let Some(entry) = entries
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["pluginId"] == stable_id)
    else {
        return Ok(None);
    };
    let conflict = entry["conflictingPluginIds"]
        .as_array()
        .is_some_and(|ids| !ids.is_empty());
    let status = if conflict {
        "conflict"
    } else if entry["enabled"] == true {
        "ready"
    } else {
        "disabled"
    };
    let display = reference::display_by_plugin_id(workspace, cancel).await?;
    let mut out = json!({
        "stableId": stable_id,
        "status": status,
        "marketplace": OFFICIAL_MARKETPLACE,
        "pluginName": plugin_name,
        "sourceTrust": "official",
    });
    if let Some(icon) = display.get(stable_id).and_then(|d| d.get("icon")) {
        out["icon"] = icon.clone();
    }
    out["diagnostics"] = if conflict {
        json!([diagnostic(
            stable_id,
            "plugin_suggested_reference_conflict",
            "推荐插件存在同名冲突，不能自动安装或引用",
        )])
    } else {
        json!([])
    };
    Ok(Some(out))
}
