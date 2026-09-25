//! 官方插件缓存写入：目录锁、marker、临时目录替换、runtime manifest 与旧版本回落。
//! 逐条对齐 TS `bundled-plugins.ts`、`official-plugin-seed-lock.ts`、`official-plugin-cache-fs.ts`、
//! `official-plugin-runtime.ts`。见 docs/specs/rust-official-plugin-seed.md。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use zcode_cli_domain::json_order::Json;

use super::official_plugins::{Definition, Host, SeedPlugin, assets};
use super::official_plugins_lock::{is_not_found, is_transient, remove_dir, retry, with_lock};

const SEED_MARKER_FILE: &str = ".zcode-plugin-seed.json";
const LOCK_BUDGET: Duration = Duration::from_secs(15);

pub(crate) fn cache_root(storage: &Path, definition: &Definition) -> PathBuf {
    storage
        .join("cache")
        .join(&assets().marketplace)
        .join(&definition.name)
        .join(&definition.version)
}

/// TS `seedBundledOfficialPlugins` 的逐插件循环；返回失败（已降级）的插件定义。
pub(crate) fn seed_plugins<'a>(
    storage: &Path,
    plugins: &'a [SeedPlugin<'a>],
    host: Option<&Host>,
) -> Result<Vec<&'a Definition>> {
    let deadline = Instant::now() + LOCK_BUDGET;
    let mut failed = Vec::new();
    for plugin in plugins {
        let target = cache_root(storage, plugin.definition);
        if !plugin.missing.is_empty() {
            eprintln!(
                "zcode-cli-rust: official plugin {} is missing required seed assets: {}",
                plugin.definition.name,
                plugin.missing.join(", ")
            );
            failed.push(plugin.definition);
            continue;
        }
        let result = with_lock(&target, deadline, || seed_one(&target, plugin, host));
        if let Err(error) = result {
            let degraded = is_transient(&error)
                || error
                    .to_string()
                    .contains("[official-plugin-seed-lock] timed out")
                || (is_not_found(&error) && is_current(&target, plugin));
            if !degraded {
                return Err(error);
            }
            eprintln!(
                "zcode-cli-rust: official plugin cache operation degraded for {}: {error:#}",
                plugin.definition.name
            );
            failed.push(plugin.definition);
        }
    }
    Ok(failed)
}

fn seed_one(target: &Path, plugin: &SeedPlugin, host: Option<&Host>) -> Result<()> {
    // 多进程并发预热：拿锁后二次检查，直接复用首个进程已提交的完整缓存。
    if is_current(target, plugin) {
        cleanup_legacy_backup(target)?;
        match write_runtime_manifest(target, &plugin.definition.name, host) {
            Ok(()) => return Ok(()),
            Err(error) if is_transient(&error) => return Err(error),
            Err(_) => {}
        }
    }
    let temporary = PathBuf::from(format!(
        "{}.tmp-{}-{}",
        target.display(),
        std::process::id(),
        now_ms()
    ));
    remove_dir(&temporary)?;
    std::fs::create_dir_all(&temporary)?;
    let result = (|| -> Result<()> {
        for file in &plugin.files {
            let bytes = std::fs::read(&file.source)?;
            let hash = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes));
            if hash != file.sha256 {
                bail!(
                    "Bundled plugin asset hash mismatch: {}/{}",
                    plugin.definition.name,
                    file.path
                );
            }
            let output = file
                .path
                .split('/')
                .fold(temporary.clone(), |p, s| p.join(s));
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&output, &bytes)?;
            set_mode(&output, file.mode)?;
        }
        std::fs::write(temporary.join(SEED_MARKER_FILE), marker(plugin).pretty())?;
        replace_root(&temporary, target, plugin)?;
        write_runtime_manifest(target, &plugin.definition.name, host)
    })();
    if result.is_err() {
        let _ = remove_dir(&temporary);
    }
    result
}

fn marker(plugin: &SeedPlugin) -> Json {
    let mut marker = Json::object();
    marker.set("hash", Json::str(&plugin.hash));
    marker.set("marketplace", Json::str(&assets().marketplace));
    marker.set("plugin", Json::str(&plugin.definition.name));
    marker.set("pluginVersion", Json::str(&plugin.definition.version));
    marker.set("source", Json::str("filesystem"));
    marker.set("version", Json::Number(1.into()));
    marker
}

fn is_current(target: &Path, plugin: &SeedPlugin) -> bool {
    std::fs::read_to_string(target.join(SEED_MARKER_FILE))
        .ok()
        .and_then(|text| Json::parse(&text))
        .is_some_and(|marker| {
            marker.get("hash").and_then(Json::as_str) == Some(plugin.hash.as_str())
                && marker.get("pluginVersion").and_then(Json::as_str)
                    == Some(plugin.definition.version.as_str())
        })
}

/// TS `replaceSeedRoot`：唯一 backup 名，失败时只恢复自己移走的 target。
fn replace_root(temporary: &Path, target: &Path, plugin: &SeedPlugin) -> Result<()> {
    let backup = PathBuf::from(format!(
        "{}.backup-{}-{}",
        target.display(),
        std::process::id(),
        now_ms()
    ));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut moved = false;
    if target.exists() {
        match retry(|| std::fs::rename(target, &backup)) {
            Ok(()) => moved = true,
            Err(error) if is_not_found(&error) => {}
            Err(error) => return Err(error),
        }
    }
    if let Err(error) = retry(|| std::fs::rename(temporary, target)) {
        if is_current(target, plugin) {
            remove_dir(temporary)?;
            if moved {
                remove_dir(&backup)?;
            }
            return cleanup_legacy_backup(target);
        }
        if moved && !target.exists() && backup.exists() {
            retry(|| std::fs::rename(&backup, target))?;
        }
        return Err(error);
    }
    if moved {
        remove_dir(&backup)?;
    }
    cleanup_legacy_backup(target)
}

fn cleanup_legacy_backup(target: &Path) -> Result<()> {
    let legacy = PathBuf::from(format!("{}.backup", target.display()));
    if legacy.exists() {
        remove_dir(&legacy)?;
    }
    Ok(())
}

/// TS `writeOfficialPluginRuntimeManifest`：把 mcpServers 改写为经 Node 插件宿主启动；无宿主时不改写。
fn write_runtime_manifest(root: &Path, name: &str, host: Option<&Host>) -> Result<()> {
    let path = root.join(".zcode-plugin").join("plugin.json");
    let current = std::fs::read_to_string(&path)?;
    let Some(mut manifest) = Json::parse(&current) else {
        bail!(
            "Official plugin manifest is not valid JSON: {}",
            path.display()
        );
    };
    let Some(servers) = manifest.get("mcpServers").cloned() else {
        return Ok(());
    };
    let Some(host) = host else { return Ok(()) };
    let Json::Object(entries) = servers else {
        bail!("Official plugin manifest has invalid mcpServers.");
    };
    let assets = assets();
    let server_path = root.join("dist").join("mcp").join("server.js");
    let mut rewritten = Json::object();
    for (key, server) in entries {
        if !server.is_object() {
            bail!("Official plugin manifest has invalid mcpServers.");
        }
        let mut server = server;
        let mut env = server
            .get("env")
            .filter(|e| e.is_object())
            .cloned()
            .unwrap_or_else(Json::object);
        server.set("command", Json::str(&host.exec_path));
        server.set(
            "args",
            Json::Array(vec![
                Json::str(&host.entrypoint),
                Json::str(&assets.host_command),
                Json::str(server_path.to_string_lossy()),
            ]),
        );
        env.set("ELECTRON_RUN_AS_NODE", Json::str("1"));
        env.set(
            &assets.plugin_id_env,
            Json::str(format!("{name}@{}", assets.marketplace)),
        );
        server.set("env", env);
        rewritten.set(&key, server);
    }
    manifest.set("mcpServers", rewritten);
    let next = format!("{}\n", manifest.pretty());
    // 同内容不触碰文件，避免放大 Windows 杀毒/索引器的占用。
    if next == current {
        return Ok(());
    }
    let temporary = path.with_file_name(format!(".tmp-{}-{}", std::process::id(), now_ms()));
    let result = retry(|| std::fs::write(&temporary, &next))
        .and_then(|()| retry(|| std::fs::rename(&temporary, &path)));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// TS `findUsableOfficialPluginFallback`：当前版本不可用时，取同名插件下最新的可用旧版本目录。
pub(crate) fn usable_fallback(storage: &Path, definition: &Definition) -> Option<PathBuf> {
    let target = cache_root(storage, definition);
    if usable(&target, definition) {
        return None;
    }
    let parent = target.parent()?;
    let mut names: Vec<String> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            *n != definition.version
                && !n.contains(".backup")
                && !n.contains(".seed-lock")
                && !n.contains(".tmp-")
        })
        .collect();
    names.sort_by(|a, b| numeric_desc(a, b));
    names
        .into_iter()
        .map(|n| parent.join(n))
        .find(|root| usable(root, definition))
}

fn usable(root: &Path, definition: &Definition) -> bool {
    let manifest = std::fs::read_to_string(root.join(".zcode-plugin").join("plugin.json"))
        .ok()
        .and_then(|t| Json::parse(&t));
    manifest.is_some_and(|m| m.get("name").and_then(Json::as_str) == Some(definition.name.as_str()))
        && definition.required.iter().all(|p| {
            p.split('/')
                .fold(root.to_owned(), |acc, s| acc.join(s))
                .exists()
        })
}

/// `localeCompare(b, a, undefined, {numeric: true, sensitivity: "base"})` 的版本目录名近似：数字段按数值比较。
fn numeric_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        s.split(|c: char| !c.is_ascii_alphanumeric())
            .map(|part| {
                part.parse::<u64>()
                    .map_or((1, 0, part.to_lowercase()), |n| (0, n, String::new()))
            })
            .collect::<Vec<_>>()
    };
    key(b).cmp(&key(a))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    // Windows 不保留 POSIX 执行位（TS chmodSync 同样只影响只读位）。
    Ok(())
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
