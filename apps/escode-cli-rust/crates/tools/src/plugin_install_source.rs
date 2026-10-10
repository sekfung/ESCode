//! 插件源物化（TS `resolvePluginSourceRoot`）：zip / 仓库类源下载到临时目录，本地源直接定位。
#[allow(unused_imports)]
use super::plugin_install::*;

use super::plugin_marketplace::sanitize;
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 物化出的插件源根；`temp` 是远端源的临时目录（用完删除）。
pub(super) struct SourceRoot {
    pub path: PathBuf,
    pub temp: Option<PathBuf>,
    pub zip: bool,
}

impl SourceRoot {
    pub(super) fn cleanup(&self) {
        if let Some(temp) = &self.temp {
            let _ = std::fs::remove_dir_all(temp);
        }
    }
}

/// TS `resolvePluginSourceRoot`：zip / 仓库类源下载到临时目录，本地源直接定位（须在阻塞线程里调用）。
/// `local`：尚未落盘的市场（目录 + manifest 原文），相对源按它解析。
pub(super) fn materialize(
    storage: &Path,
    marketplace: &str,
    entry: &Json,
    local: Option<(&Path, &Json)>,
) -> Result<SourceRoot> {
    if is_zip_source(entry) {
        let source = entry.get("source").cloned().unwrap_or(Json::Null);
        let root =
            tokio::runtime::Handle::current().block_on(super::plugin_zip::resolve(&source))?;
        return Ok(SourceRoot {
            path: root.path.clone(),
            temp: Some(root.temp_dir()),
            zip: true,
        });
    }
    if let Some(repo) = repository_source(entry)? {
        let root = tokio::runtime::Handle::current().block_on(super::plugin_git::resolve(repo))?;
        return Ok(SourceRoot {
            path: root.path.clone(),
            temp: Some(root.temp_dir()),
            zip: false,
        });
    }
    let path = match local {
        Some((dir, manifest)) => {
            source_root_in(storage, marketplace, dir, Some(manifest.clone()), entry)?
        }
        None => source_root(storage, marketplace, entry)?,
    };
    Ok(SourceRoot {
        path,
        temp: None,
        zip: false,
    })
}

/// TS `resolvePluginSourceRoot`（本地源部分）。
pub(super) fn source_root(storage: &Path, marketplace: &str, entry: &Json) -> Result<PathBuf> {
    let marketplace_dir = storage.join("marketplaces").join(sanitize(marketplace));
    let manifest = ordered_manifest(storage, marketplace);
    source_root_in(storage, marketplace, &marketplace_dir, manifest, entry)
}

pub(super) fn source_root_in(
    storage: &Path,
    marketplace: &str,
    marketplace_dir: &Path,
    manifest: Option<Json>,
    entry: &Json,
) -> Result<PathBuf> {
    let name = entry.get("name").and_then(Json::as_str).unwrap_or_default();
    let id = format!("{name}@{marketplace}");
    let marketplace_dir = marketplace_dir.to_owned();
    let plugin_root = manifest
        .and_then(|m| m.get("metadata").cloned())
        .and_then(|meta| {
            meta.get("pluginRoot")
                .and_then(Json::as_str)
                .map(str::to_owned)
        });
    let base = plugin_root
        .and_then(|root| resolve_inside(&marketplace_dir, &root))
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| marketplace_dir.clone());
    match entry.get("source") {
        Some(Json::String(kind)) if kind == "filesystem" || kind == "sea" => {
            if let Some(cache) = entry
                .get("cachePath")
                .and_then(Json::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
            {
                return Ok(cache);
            }
            let version = entry
                .get("version")
                .and_then(Json::as_str)
                .unwrap_or("0.0.0");
            let computed = cache_dir(storage, marketplace, name, version);
            if computed.is_dir() {
                return Ok(computed);
            }
            bail!("Bundled plugin cache directory missing: {id}")
        }
        Some(Json::String(path)) => {
            let relative = path.strip_prefix("./").unwrap_or(path);
            if let Some(local) = resolve_inside(&base, relative).filter(|p| p.is_dir()) {
                return Ok(local);
            }
            let absolute = PathBuf::from(path);
            if absolute.is_absolute() && absolute.is_dir() {
                return Ok(absolute);
            }
            bail!("Unsupported or missing plugin source: {path}")
        }
        Some(source @ Json::Object(_)) => {
            let kind = source
                .get("source")
                .and_then(Json::as_str)
                .unwrap_or_default();
            match kind {
                "directory" => {
                    let path = source
                        .get("path")
                        .and_then(Json::as_str)
                        .filter(|p| !p.trim().is_empty())
                        .ok_or_else(|| {
                            anyhow!("Plugin directory path source requires a non-empty path")
                        })?;
                    let path = super::lexical_path::normalize(Path::new(path));
                    if path.is_dir() {
                        return Ok(path);
                    }
                    bail!("Plugin source directory does not exist: {}", path.display())
                }
                // 仓库类与 zip 源在 source_root 之前已分流；剩下的 url:<其它 type> 与 npm / pip 同 TS 不支持。
                "url" | "npm" | "pip" => {
                    let label = if kind == "url" {
                        match source.get("type").and_then(Json::as_str) {
                            Some(t) if !t.is_empty() => format!("url:{t}"),
                            _ => "url".to_owned(),
                        }
                    } else {
                        kind.to_owned()
                    };
                    bail!("Plugin source is recognized but not supported in this runtime: {label}")
                }
                _ => bail!(
                    "Plugin source is invalid or unsupported for {id}: {}",
                    if kind.is_empty() {
                        "missing kind"
                    } else {
                        kind
                    }
                ),
            }
        }
        _ => {
            let local = base.join(name);
            if local.is_dir() {
                return Ok(local);
            }
            bail!("Plugin source is not supported for {id}")
        }
    }
}

/// 仓库类源（TS `resolvePluginSourceRoot` 的 github / git / url(git) / git-subdir 分支）；字段校验文案同 TS。
pub(super) fn repository_source(entry: &Json) -> Result<Option<super::plugin_git::RepoSource>> {
    let Some(source) = entry.get("source").filter(|s| s.is_object()) else {
        return Ok(None);
    };
    let text = |key: &str| source.get(key).and_then(Json::as_str).map(str::to_owned);
    let required = |key: &str, label: &str| {
        text(key)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow!("Plugin {label} source requires a non-empty {key}"))
    };
    let kind = text("source").unwrap_or_default();
    // TS readPluginSourceIdentityPin：仓库源没有 zip sha256，取 sha，其次旧写法 commit。
    let sha = text("sha").or_else(|| text("commit"));
    let repo = match kind.as_str() {
        "github" => {
            let repo = required("repo", "GitHub repo")?;
            super::plugin_git::RepoSource {
                url: format!("https://github.com/{repo}.git"),
                path: text("path"),
                r#ref: text("ref"),
                sha,
            }
        }
        "git" => super::plugin_git::RepoSource {
            url: required("url", "Git URL")?,
            path: text("path"),
            r#ref: text("ref"),
            sha,
        },
        "url" => {
            let url = required("url", "URL")?;
            match text("type").unwrap_or_default().as_str() {
                "" | "git" => super::plugin_git::RepoSource {
                    url,
                    path: text("path"),
                    r#ref: text("ref"),
                    sha,
                },
                _ => return Ok(None),
            }
        }
        "git-subdir" => {
            let path = required("path", "git-subdir path")?;
            let url = required("url", "git-subdir URL")?;
            // TS normalizeGitUrl：owner/repo 简写补成 GitHub HTTPS。
            let is_short = url.split('/').count() == 2
                && !url.contains(':')
                && url.split('/').all(|part| !part.is_empty());
            super::plugin_git::RepoSource {
                url: if is_short {
                    format!("https://github.com/{url}.git")
                } else {
                    url
                },
                path: Some(path),
                r#ref: text("ref"),
                sha,
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(repo))
}

/// `{source: "url", type: "zip"}`（TS `isZipPluginUrlSource` 的入口判定；字段校验在 plugin_zip）。
pub(super) fn is_zip_source(entry: &Json) -> bool {
    let Some(source) = entry.get("source").filter(|s| s.is_object()) else {
        return false;
    };
    source.get("source").and_then(Json::as_str) == Some("url")
        && source.get("type").and_then(Json::as_str) == Some("zip")
}

/// TS `assertZipPluginInstallRoot` + `readPluginManifestFromRoot`：多顶层 zip 未指定 path 时会回退到解压根，
/// 必须确认它能形成合法插件且名字与目录条目一致，否则不能写安装记录。
pub(super) fn assert_zip_root(root: &Path, entry: &Json, marketplace: &str) -> Result<()> {
    let entry_name = entry.get("name").and_then(Json::as_str).unwrap_or_default();
    let id = format!("{entry_name}@{marketplace}");
    let name = match manifest_path(root) {
        Some(path) => {
            let parsed: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            if !parsed.is_object() {
                bail!("Plugin manifest must be a JSON object");
            }
            let name = parsed["name"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_owned();
            if !valid_plugin_name(&name) {
                bail!("Invalid plugin name: {name}");
            }
            name
        }
        None if entry.get("strict") == Some(&Json::Bool(false)) => entry_name.trim().to_owned(),
        None => bail!("Plugin manifest not found: {id}"),
    };
    if name != entry_name {
        bail!("Plugin manifest name '{name}' does not match marketplace entry '{entry_name}'");
    }
    Ok(())
}
