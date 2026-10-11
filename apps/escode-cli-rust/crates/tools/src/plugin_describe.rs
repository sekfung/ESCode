//! `plugins/describe`（docs/specs/rust-plugin-marketplace-write.md W5b）：对齐 TS `describePlugin` →
//! adapters `describeMarketplacePlugin` / `readComponentsAtRoot` / `enumeratePluginComponents`。
//! 已安装插件读本地安装目录；未安装候选按源物化到临时目录（用完即删）后枚举组件名称与描述。

use super::plugin_install as install;
use super::plugin_marketplace as market;
use super::plugin_uninstall::{read_installed_sync, storage_lock};
use super::plugin_validate::{diag, entry_name, error_diagnostic};
use super::plugin_validate_mcp::mcp_definitions;
use super::plugin_validate_shape::read_root_manifest;
use super::{extension_config as config, plugin_list};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[allow(unused_imports)]
pub(super) use super::plugin_describe_scan::{hook_events, markdown_components, skill_components};
#[allow(unused_imports)]
pub(super) use super::plugin_frontmatter::read_frontmatter;

pub(super) async fn describe(params: &Value) -> Result<Value> {
    let cwd = plugin_list::workspace_path(params)?;
    let name = plugin_list::non_empty(params, "pluginName")?.to_owned();
    let marketplace = plugin_list::non_empty(params, "marketplace")?.to_owned();
    let config = config::load(&cwd).await?;
    let storage = config::storage(&config);
    let _guard = storage_lock(&storage).await;
    market::ensure_default_marketplaces(&storage)?;
    let id = format!("{name}@{marketplace}");

    // 已安装优先：本地目录无需联网；记录在但目录缺失时继续走源解析兜底。
    let installed = read_installed_sync(&storage).into_iter().find(|record| {
        let field = |key: &str| record.get(key).and_then(Json::as_str);
        field("marketplace") == Some(marketplace.as_str()) && field("name") == Some(name.as_str())
    });
    if let Some(record) = installed {
        let root = installed_root(&storage, &record);
        if root.is_dir() {
            let mut diagnostics = vec![];
            let read = read_components(&root, None, &marketplace, &mut diagnostics);
            return Ok(result(read, diagnostics));
        }
    }

    if let Err(error) = super::plugin_market_write::ensure_manifest(&storage, &marketplace).await {
        return Ok(result(
            (vec![], None),
            vec![error_diagnostic(&error, Some(&id))],
        ));
    }
    if market::manifest(&storage, &marketplace).is_none() {
        let message = format!("Marketplace not found: {marketplace}");
        return Ok(result(
            (vec![], None),
            vec![diag("plugin_marketplace_invalid", message, None, true)],
        ));
    }
    let Some(entry) = install::ordered_entry(&storage, &marketplace, &name) else {
        let message = format!("Plugin not found: {id}");
        return Ok(result(
            (vec![], None),
            vec![diag("plugin_not_found", message, None, true)],
        ));
    };
    tokio::task::spawn_blocking(move || {
        let mut diagnostics = vec![];
        match install::materialize(&storage, &marketplace, &entry, None) {
            Ok(root) => {
                let read =
                    read_components(&root.path, Some(&entry), &marketplace, &mut diagnostics);
                root.cleanup();
                result(read, diagnostics)
            }
            Err(error) => result((vec![], None), vec![error_diagnostic(&error, Some(&id))]),
        }
    })
    .await
    .map_err(|_| anyhow!("Plugin describe worker panicked"))
}

type Components = (Vec<Value>, Option<Value>);

fn result((components, metadata): Components, diagnostics: Vec<Value>) -> Value {
    let mut out = json!({ "components": components });
    if !diagnostics.is_empty() {
        out["diagnostics"] = Value::Array(diagnostics);
    }
    if let Some(metadata) = metadata {
        out["metadata"] = metadata;
    }
    out
}

/// TS `resolveInstalledPluginRoot`：记录的 installPath，缺省按缓存目录约定；恢复中断的原子激活。
fn installed_root(storage: &Path, record: &Json) -> PathBuf {
    let field = |key: &str| {
        record
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let path = field("installPath");
    let root = if path.is_empty() {
        storage
            .join("cache")
            .join(market::sanitize(&field("marketplace")))
            .join(market::sanitize(&field("name")))
            .join(market::sanitize(&field("version")))
    } else {
        PathBuf::from(path)
    };
    super::atomic_dir::recover(&root)
}

/// TS `readComponentsAtRoot`：manifest 读取失败按 null 降级，仍按默认目录约定扫描。
fn read_components(
    root: &Path,
    entry: Option<&Json>,
    marketplace: &str,
    diagnostics: &mut Vec<Value>,
) -> Components {
    let placeholder = Json::parse(r#"{"name":"__describe__"}"#).unwrap_or_else(Json::object);
    let manifest = read_root_manifest(root, entry.unwrap_or(&placeholder))
        .ok()
        .flatten();
    let field = |key: &str| manifest.as_ref().and_then(|m| m.get(key));
    let mut groups = vec![];
    let mut push = |kind: &str, items: Vec<Value>| {
        if !items.is_empty() {
            groups.push(json!({ "kind": kind, "items": items }));
        }
    };
    push(
        "agent",
        markdown_components(root, field("agents"), "agents"),
    );
    push(
        "command",
        markdown_components(root, field("commands"), "commands"),
    );
    push("skill", skill_components(root, field("skills")));
    if let Some(manifest) = &manifest {
        let id = format!("{}@{marketplace}", entry_name(manifest));
        let hooks = hook_events(root, manifest, &id, diagnostics);
        push(
            "hook",
            hooks
                .into_iter()
                .map(|name| json!({ "name": name }))
                .collect(),
        );
        let servers = mcp_definitions(root, manifest, &id, diagnostics);
        push(
            "mcp",
            servers
                .into_iter()
                .map(|(name, _)| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .map(|name| json!({ "name": name }))
                .collect(),
        );
    }
    let metadata = manifest.as_ref().and_then(display_metadata);
    (groups, metadata)
}

/// TS `toManifestDisplayMetadata` + `normalizeAuthorValue`。
fn display_metadata(manifest: &Json) -> Option<Value> {
    let trimmed = |value: Option<&Json>| {
        value
            .and_then(Json::as_str)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let (author, author_url) = match manifest.get("author") {
        Some(Json::String(name)) => (trimmed(Some(&Json::str(name.clone()))), None),
        Some(author @ Json::Object(_)) => (trimmed(author.get("name")), trimmed(author.get("url"))),
        _ => (None, None),
    };
    let mut out = serde_json::Map::new();
    if let Some(author) = author {
        out.insert("author".into(), author.into());
    }
    if let Some(url) = author_url {
        out.insert("authorUrl".into(), url.into());
    }
    if let Some(homepage) = manifest
        .get("homepage")
        .and_then(Json::as_str)
        .filter(|h| !h.trim().is_empty())
    {
        out.insert("homepage".into(), homepage.into());
    }
    if let Some(version) = manifest
        .get("version")
        .and_then(Json::as_str)
        .filter(|v| !v.is_empty())
    {
        out.insert("version".into(), version.into());
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

// ---- Markdown frontmatter（TS plugins/markdown-frontmatter.ts：只取 name / description，支持块标量） ----
