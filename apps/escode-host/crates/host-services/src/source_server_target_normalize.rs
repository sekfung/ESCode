//! `ensureProfile` / `updateProfile` 目标归一化：只保留 settings 严格 schema 允许的字段（spec §9.8/§12.2）。

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

pub(super) fn kind_of(target: &Value) -> &str {
    target
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// 目标归一化为 settings 的严格 schema；未知枚举/扩展字段不写入。
pub(super) fn normalize_target(target: &Value) -> Result<Value> {
    let object = target
        .as_object()
        .context("Connection target must be an object")?;
    let mut normalized = Map::new();
    match kind_of(target) {
        "ssh" => normalize_ssh(object, &mut normalized)?,
        "wsl" => normalize_wsl(object, &mut normalized)?,
        "docker" => {
            let container = string_field(object, "container")
                .map(str::trim)
                .unwrap_or_default();
            if container.is_empty() {
                bail!("Docker container is required");
            }
            normalized.insert("kind".to_owned(), Value::from("docker"));
            normalized.insert("container".to_owned(), Value::from(container));
        }
        other => bail!("Unsupported connection transport: {other}"),
    }
    Ok(Value::Object(normalized))
}

fn normalize_ssh(object: &Map<String, Value>, normalized: &mut Map<String, Value>) -> Result<()> {
    let host = string_field(object, "host")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let username = string_field(object, "username")
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    if host.is_empty() || username.is_empty() {
        bail!("SSH connection details are invalid");
    }
    let port = match object.get("port") {
        None | Some(Value::Null) => 22,
        Some(value) => value
            .as_u64()
            .filter(|port| (1..=65_535).contains(port))
            .context("SSH connection details are invalid")?,
    };
    normalized.insert("kind".to_owned(), Value::from("ssh"));
    normalized.insert("host".to_owned(), Value::from(host));
    normalized.insert("port".to_owned(), Value::from(port));
    normalized.insert("username".to_owned(), Value::from(username));
    for key in ["sshConfigAlias", "privateKeyPath"] {
        if let Some(value) = string_field(object, key).filter(|value| !value.is_empty()) {
            normalized.insert(key.to_owned(), Value::from(value));
        }
    }
    if let Some(mode) = string_field(object, "assetInstallMode")
        && matches!(mode, "local-download-upload" | "remote-download")
    {
        normalized.insert("assetInstallMode".to_owned(), Value::from(mode));
    }
    if let Some(packages) = normalized_resource_packages(object.get("resourcePackages")) {
        normalized.insert("resourcePackages".to_owned(), packages);
    }
    Ok(())
}

fn normalize_wsl(object: &Map<String, Value>, normalized: &mut Map<String, Value>) -> Result<()> {
    normalized.insert("kind".to_owned(), Value::from("wsl"));
    if let Some(distro) = string_field(object, "distro").map(str::trim)
        && !distro.is_empty()
    {
        normalized.insert("distro".to_owned(), Value::from(distro));
    }
    if let Some(user) = string_field(object, "user").map(str::trim)
        && !user.is_empty()
    {
        if !is_valid_wsl_user(user) {
            bail!("WSL user is invalid");
        }
        normalized.insert("user".to_owned(), Value::from(user));
    }
    Ok(())
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

const KNOWN_RESOURCE_PACKAGE_IDS: [&str; 7] = [
    "server-bundle",
    "node-runtime",
    "node-pty",
    "glm",
    "bfs",
    "ripgrep",
    "ugrep",
];

/// 与 shared `sshProfileTargetSchema.resourcePackages` 对齐；不合法整键丢弃（当前 no-op）。
fn normalized_resource_packages(value: Option<&Value>) -> Option<Value> {
    let object = value?.as_object()?;
    if object.keys().any(|key| key != "selectedPackageIds") {
        return None;
    }
    let mut normalized = Map::new();
    if let Some(ids) = object.get("selectedPackageIds") {
        let mut selected = Vec::new();
        for id in ids.as_array()? {
            let id = id.as_str()?;
            if !KNOWN_RESOURCE_PACKAGE_IDS.contains(&id) {
                return None;
            }
            selected.push(Value::from(id));
        }
        normalized.insert("selectedPackageIds".to_owned(), Value::Array(selected));
    }
    Some(Value::Object(normalized))
}

/// shared `isValidWslUser`：≤64、无控制字符、不含 `:` `/` `\`。
fn is_valid_wsl_user(value: &str) -> bool {
    let user = value.trim();
    (1..=64).contains(&user.chars().count())
        && !user
            .chars()
            .any(|character| (character as u32) < 32 || character == '\u{7f}')
        && !user.contains([':', '/', '\\'])
}
