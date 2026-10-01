//! `plugins/validate` 的条目形状、manifest 读取与兼容性诊断（TS `validateMarketplaceEntryShape` /
//! `getMarketplaceSourceValidationDeferral` / `pushManifestCompatibilityDiagnostics` / `validatePluginRoot`）。

use super::plugin_install as install;
use super::plugin_validate::{diag, entry_name};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::path::Path;

const UNSUPPORTED_MANIFEST_FIELDS: [&str; 4] =
    ["channels", "lspServers", "outputStyles", "settings"];

/// TS `validateMarketplaceEntryShape`（includeEntryCompatibility:false）。
pub(super) fn entry_shape(entry: &Json, id: &str) -> Vec<Value> {
    let mut out = vec![];
    let Some(source) = entry.get("source") else {
        return vec![diag(
            "plugin_marketplace_invalid",
            format!("Plugin has no install source: {id}"),
            Some(id),
            true,
        )];
    };
    if !source.is_object() {
        return out;
    }
    let text = |key: &str| source.get(key).and_then(Json::as_str).unwrap_or_default();
    let kind = text("source");
    if kind == "npm" || kind == "pip" {
        out.push(diag(
            "plugin_marketplace_source_unsupported",
            format!("Plugin source is recognized but not supported in V1 install: {kind}"),
            Some(id),
            false,
        ));
    }
    if kind == "url" {
        let source_type = text("type");
        if !source_type.is_empty() && source_type != "git" && source_type != "zip" {
            out.push(diag(
                "plugin_marketplace_source_unsupported",
                format!("Plugin URL source type is not supported: {source_type}"),
                Some(id),
                true,
            ));
        }
        if let Err(message) = url_shape(source, source_type == "zip") {
            out.push(diag("plugin_marketplace_invalid", message, Some(id), true));
        }
    }
    out
}

/// TS 形状校验里的 `readRequired*` / `readOptional*`（zip 源：url → sha256 → headers → path → stripRoot）。
fn url_shape(source: &Json, zip: bool) -> Result<(), String> {
    let has_text = |key: &str| {
        source
            .get(key)
            .and_then(Json::as_str)
            .is_some_and(|v| !v.trim().is_empty())
    };
    if !has_text("url") {
        return Err("Plugin URL source requires a non-empty url".into());
    }
    if !zip {
        return Ok(());
    }
    let Some(sha) = source.get("sha256").and_then(Json::as_str) else {
        return Err("Plugin zip source sha256 is required".into());
    };
    let sha = sha.to_lowercase();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Plugin zip source sha256 must be a 64 character hex string".into());
    }
    match source.get("headers") {
        None => {}
        Some(Json::Object(entries)) => {
            if let Some((key, _)) = entries.iter().find(|(_, v)| v.as_str().is_none()) {
                return Err(format!("Plugin zip source header must be a string: {key}"));
            }
        }
        Some(_) => return Err("Plugin zip source headers must be an object".into()),
    }
    if source.get("path").is_some_and(|p| p.as_str().is_none()) {
        return Err("Plugin zip source path must be a string".into());
    }
    if source
        .get("stripRoot")
        .is_some_and(|v| !matches!(v, Json::Bool(_)))
    {
        return Err("Plugin zip source stripRoot must be a boolean".into());
    }
    Ok(())
}

/// TS `getMarketplaceSourceValidationDeferral`：市场级校验不逐个 clone 远端源。
pub(super) fn deferral(entry: &Json, id: &str) -> Option<Value> {
    let source = entry.get("source").filter(|s| s.is_object())?;
    let text = |key: &str| source.get(key).and_then(Json::as_str);
    let kind = text("source").unwrap_or_default();
    if kind == "url" {
        let source_type = text("type").unwrap_or_default();
        if !source_type.is_empty() && source_type != "git" && source_type != "zip" {
            return None;
        }
    }
    if !["github", "git", "url", "git-subdir"].contains(&kind) {
        return None;
    }
    let label = text("repo").or_else(|| text("url")).unwrap_or(kind);
    Some(diag(
        "plugin_validation_deferred",
        format!(
            "Remote plugin source validation is deferred until install or single-plugin validate: {label}"
        ),
        Some(id),
        false,
    ))
}

/// TS `createManifestFromMarketplaceEntry`：条目去掉来源 / 商店展示字段，补 name / version。
pub(super) fn entry_manifest(entry: &Json) -> Json {
    let mut manifest = entry.clone();
    for key in [
        "source",
        "category",
        "tags",
        "strict",
        "displayName",
        "displayName_i18n",
        "description_i18n",
        "icon",
        "privacyPolicy",
        "termsOfService",
        "heroImage",
        "examplePrompts",
        "examplePrompts_i18n",
        "requiresPaidPlan",
    ] {
        manifest.remove(key);
    }
    manifest.set("name", Json::str(entry_name(entry)));
    let version = entry
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or("0.0.0")
        .to_owned();
    manifest.set("version", Json::str(version));
    manifest
}

/// TS `pushManifestCompatibilityDiagnostics`。
pub(super) fn compatibility(manifest: &Json, id: &str) -> Vec<Value> {
    let mut out = vec![];
    for key in UNSUPPORTED_MANIFEST_FIELDS {
        if manifest.get(key).is_some() {
            out.push(diag(
                "plugin_unsupported_component",
                format!("Plugin component is diagnostic-only in this ZCode runtime: {key}"),
                Some(id),
                false,
            ));
        }
    }
    if let Some(Json::Object(options)) = manifest.get("userConfig") {
        for (key, option) in options {
            if option.get("required") == Some(&Json::Bool(true)) && option.get("default").is_none()
            {
                out.push(diag(
                    "plugin_variable_missing",
                    format!(
                        "Required plugin userConfig has no default and must be configured: {key}"
                    ),
                    Some(id),
                    false,
                ));
            }
        }
    }
    fn bundle(value: &Json) -> bool {
        match value {
            Json::String(text) => text.ends_with(".mcpb") || text.ends_with(".dxt"),
            Json::Array(items) => items.iter().any(bundle),
            _ => false,
        }
    }
    if manifest.get("mcpServers").is_some_and(bundle) {
        out.push(diag(
            "plugin_marketplace_source_unsupported",
            "MCPB/DXT plugin bundles are recognized but not supported in this runtime",
            Some(id),
            false,
        ));
    }
    out
}

/// TS `readPluginManifestFromRoot`：有 plugin.json 则解析（name trim + 校验，version 缺省 0.0.0）；
/// 没有且条目 `strict: false` 时用条目合成；否则 None。
pub(super) fn read_root_manifest(root: &Path, entry: &Json) -> Result<Option<Json>> {
    if let Some(path) = install::manifest_path(root) {
        let text = std::fs::read_to_string(&path)?;
        let mut parsed = Json::parse(&text)
            .ok_or_else(|| anyhow!("Unexpected token in JSON at {}", path.display()))?;
        if !parsed.is_object() {
            return Err(anyhow!("Plugin manifest must be a JSON object"));
        }
        let name = entry_name(&parsed);
        if !install::valid_plugin_name(&name) {
            return Err(anyhow!("Invalid plugin name: {name}"));
        }
        parsed.set("name", Json::str(name));
        if parsed.get("version").and_then(Json::as_str).is_none() {
            parsed.set("version", Json::str("0.0.0"));
        }
        return Ok(Some(parsed));
    }
    if entry.get("strict") == Some(&Json::Bool(false)) {
        return Ok(Some(entry_manifest(entry)));
    }
    Ok(None)
}

/// TS `validatePluginRoot`。
pub(super) fn validate_root(entry: &Json, marketplace: &str, root: &Path) -> Vec<Value> {
    let name = entry_name(entry);
    let id = format!("{name}@{marketplace}");
    let manifest = match read_root_manifest(root, entry) {
        Err(error) => {
            return vec![diag(
                "plugin_manifest_invalid",
                error.to_string(),
                Some(&id),
                true,
            )];
        }
        Ok(None) => {
            return vec![diag(
                "plugin_manifest_not_found",
                format!("Plugin manifest not found: {id}"),
                Some(&id),
                true,
            )];
        }
        Ok(Some(manifest)) => manifest,
    };
    let mut out = vec![];
    let manifest_name = entry_name(&manifest);
    if manifest_name != name {
        out.push(diag(
            "plugin_manifest_invalid",
            format!(
                "Plugin manifest name '{manifest_name}' does not match marketplace entry '{name}'"
            ),
            Some(&id),
            true,
        ));
    }
    out.extend(compatibility(&manifest, &id));
    out.extend(super::plugin_validate_mcp::diagnostics(
        root, &manifest, &id,
    ));
    out
}
