use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

pub(super) fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}
pub(super) fn resolve(base: &Path, path: &str) -> PathBuf {
    let raw = path
        .strip_prefix("~/")
        .map(|p| home().join(p))
        .unwrap_or_else(|| base.join(path));
    super::lexical_path::normalize(&raw)
}
pub(super) async fn json_file(path: &Path) -> Result<Value> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = vec![];
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "Extension configuration exceeds size limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
pub(super) async fn project_directories(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![];
    for dir in cwd.ancestors() {
        dirs.push(dir.to_owned());
        if tokio::fs::metadata(dir.join(".git")).await.is_ok() {
            return dirs;
        }
    }
    vec![cwd.to_owned()]
}
pub(super) async fn load(cwd: &Path) -> Result<Value> {
    Ok(load_layers(cwd).await?.merged())
}

/// 分层的配置视图：用户层与（多文件合并后的）项目层。插件页的 `enabledSource` / `optionSources` /
/// `rootSource` 要知道哪一层写了哪个值（TS `resolvePluginConfigSources`）。
pub(super) struct Layers {
    pub user: Value,
    pub project: Value,
}

impl Layers {
    /// 用户层叠加项目层后的有效配置（与 `load` 相同）。
    pub fn merged(&self) -> Value {
        let mut config = self.user.clone();
        merge(&mut config, &self.project, 0);
        config
    }
}

pub(super) async fn load_layers(cwd: &Path) -> Result<Layers> {
    let user = json_file(&home().join(".escode").join("cli").join("config.json")).await?;
    let mut config = json!({});
    for dir in project_directories(cwd).await.into_iter().rev() {
        for file in ["escode.json", ".escode/config.json"] {
            let mut next = json_file(&dir.join(file)).await?;
            if let Some(servers) = next["mcp"]["servers"].as_object_mut() {
                for server in servers.values_mut() {
                    if server["command"].is_string() {
                        server["cwd"] = resolve(&dir, server["cwd"].as_str().unwrap_or("."))
                            .to_string_lossy()
                            .into_owned()
                            .into();
                    }
                }
            }
            merge(&mut config, &next, 0);
        }
    }
    Ok(Layers {
        user,
        project: config,
    })
}

/// 只含用户层（`~/.escode/cli/config.json`）的配置视图：`plugins/list` 的 `configScope: "user"`
/// 走这条（TS `createPluginConfigView` 在 user 视图不传 workingDirectory，避免把项目 override
/// 投影成用户当前值）。
pub(super) async fn load_user() -> Result<Value> {
    json_file(&home().join(".escode").join("cli").join("config.json")).await
}
fn merge(base: &mut Value, next: &Value, depth: usize) {
    let Some(object) = next.as_object() else {
        *base = next.clone();
        return;
    };
    if !base.is_object() {
        *base = json!({});
    }
    for (key, value) in object {
        if depth == 0 && key == "plugins" && value.is_object() {
            merge_plugins(&mut base[key], value);
            continue;
        }
        // 顶层类别及服务器/插件字典按 key 合并；单个 server/override 是完整配置。
        if depth < 2 && value.is_object() {
            merge(&mut base[key], value, depth + 1);
        } else {
            base[key] = value.clone();
        }
    }
}
/// TS `mergeConfigs` 的 plugins 分支：后一层不能整体覆盖前一层——`dirs` 取并集（保序去重），
/// `enabledPlugins` / `extraKnownMarketplaces` 按 id、`options` 按 pluginId 再按 option key 合并。
fn merge_plugins(base: &mut Value, next: &Value) {
    if !base.is_object() {
        *base = json!({});
    }
    let Some(next) = next.as_object() else {
        return;
    };
    for (key, value) in next {
        match (key.as_str(), value) {
            ("dirs", Value::Array(dirs)) => {
                let mut merged = base["dirs"].as_array().cloned().unwrap_or_default();
                for dir in dirs {
                    if !merged.contains(dir) {
                        merged.push(dir.clone());
                    }
                }
                base["dirs"] = Value::Array(merged);
            }
            ("enabledPlugins" | "extraKnownMarketplaces", Value::Object(entries)) => {
                if !base[key].is_object() {
                    base[key] = json!({});
                }
                for (id, entry) in entries {
                    base[key][id] = entry.clone();
                }
            }
            ("options", Value::Object(plugins)) => {
                if !base["options"].is_object() {
                    base["options"] = json!({});
                }
                for (id, options) in plugins {
                    match options.as_object() {
                        Some(options) => {
                            if !base["options"][id].is_object() {
                                base["options"][id] = json!({});
                            }
                            for (option, value) in options {
                                base["options"][id][option] = value.clone();
                            }
                        }
                        None => base["options"][id] = options.clone(),
                    }
                }
            }
            _ => base[key] = value.clone(),
        }
    }
}

pub(super) fn storage(config: &Value) -> PathBuf {
    let base = std::env::var("ESCODE_STORAGE_DIR")
        .ok()
        .or_else(|| config["storage"]["dir"].as_str().map(str::to_owned));
    let base = base
        .map(|p| resolve(&home(), &p))
        .unwrap_or_else(|| home().join(".escode"));
    if base.file_name().is_some_and(|p| p == "cli") {
        base.join("plugins")
    } else {
        base.join("cli").join("plugins")
    }
}
pub(super) fn strings(value: &Value) -> Vec<&str> {
    if let Some(s) = value.as_str() {
        vec![s]
    } else {
        value
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }
}

/// 用户 / 项目配置里的 `permission` 段（allowedTools / disallowedTools / autoApproveHighRisk）。
pub(super) async fn permission_config(cwd: &Path) -> crate::domain::permission::Config {
    let config = load(cwd).await.unwrap_or_else(|_| serde_json::json!({}));
    let permission = &config["permission"];
    let list = |key: &str| {
        permission[key]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    crate::domain::permission::Config {
        allowed: list("allowedTools"),
        disallowed: list("disallowedTools"),
        auto_approve_high_risk: permission["autoApproveHighRisk"].as_bool() == Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_layers_merge_like_ts() {
        let mut config = json!({"plugins": {
            "dirs": ["a", "b"],
            "enabledPlugins": {"x@m": true, "y@m": false},
            "options": {"x@m": {"k1": 1, "k2": "u"}},
            "suppressedBuiltins": ["s"],
        }});
        merge(
            &mut config,
            &json!({"plugins": {
                "dirs": ["b", "c"],
                "enabledPlugins": {"y@m": true},
                "options": {"x@m": {"k2": "w"}, "z@m": {"k": true}},
            }}),
            0,
        );
        assert_eq!(
            config,
            json!({"plugins": {
                "dirs": ["a", "b", "c"],
                "enabledPlugins": {"x@m": true, "y@m": true},
                "options": {"x@m": {"k1": 1, "k2": "w"}, "z@m": {"k": true}},
                "suppressedBuiltins": ["s"],
            }})
        );
    }
}
