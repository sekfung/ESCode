//! 官方插件 seed：把随包插件写入 `<storage>/cache/zcode-plugins-official/<name>/<version>` 并维护 marketplace。
//! 与 TS `bundled-plugins.ts` 字节一致，Node 与 Rust 共享同一缓存。见 docs/specs/rust-official-plugin-seed.md。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use sha2::{Digest, Sha256};
use zcode_cli_domain::json_order::Json;

use super::official_plugins_cache as cache;

pub(crate) struct Assets {
    pub marketplace: String,
    pub host_command: String,
    pub plugin_id_env: String,
    included: Vec<String>,
    collation: Vec<char>,
    pub definitions: Vec<Definition>,
}

pub(crate) struct Definition {
    pub name: String,
    pub version: String,
    root_candidates: Vec<String>,
    pub required: Vec<String>,
    runtime_top_level: Vec<String>,
    pub listing: Option<Json>,
}

pub(crate) struct SeedFile {
    pub path: String,
    pub source: PathBuf,
    pub sha256: String,
    pub mode: u32,
}

pub(crate) struct SeedPlugin<'a> {
    pub definition: &'a Definition,
    pub files: Vec<SeedFile>,
    pub hash: String,
    pub missing: Vec<String>,
}

/// Node 插件宿主：Electron-as-Node 可执行文件与 zcode.cjs 入口（由 Host 传入）。
pub(crate) struct Host {
    pub exec_path: String,
    pub entrypoint: String,
}

pub(crate) fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let raw = Json::parse(include_str!("official_plugins.json")).expect("generated assets");
        let text =
            |value: Option<&Json>| value.and_then(Json::as_str).unwrap_or_default().to_owned();
        let list = |value: Option<&Json>| -> Vec<String> {
            value
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Json::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Assets {
            marketplace: text(raw.get("marketplace")),
            host_command: text(raw.get("hostCommand")),
            plugin_id_env: text(raw.get("pluginIdEnvKey")),
            included: list(raw.get("includedTopLevelPaths")),
            collation: list(raw.get("collation"))
                .iter()
                .filter_map(|s| s.chars().next())
                .collect(),
            definitions: raw
                .get("definitions")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .map(|d| Definition {
                    name: text(d.get("name")),
                    version: text(d.get("version")),
                    root_candidates: list(d.get("rootCandidates")),
                    required: list(d.get("requiredSeedPaths")),
                    runtime_top_level: list(d.get("runtimeTopLevelPaths")),
                    listing: d.get("listing").cloned(),
                })
                .collect(),
        }
    })
}

/// 进程内每个 storage root 只 seed 一次；返回失败插件回落到的旧版本缓存根。
pub(crate) fn seed_once(storage: &Path) -> Vec<PathBuf> {
    static SEEDED: std::sync::Mutex<Vec<(PathBuf, Vec<PathBuf>)>> =
        std::sync::Mutex::new(Vec::new());
    let mut seeded = SEEDED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, roots)) = seeded.iter().find(|(root, _)| root == storage) {
        return roots.clone();
    }
    let host = match (
        std::env::var("ZCODE_PLUGIN_HOST_EXEC_PATH"),
        std::env::var("ZCODE_PLUGIN_HOST_ENTRYPOINT"),
    ) {
        (Ok(exec_path), Ok(entrypoint)) if !exec_path.is_empty() && !entrypoint.is_empty() => {
            Some(Host {
                exec_path,
                entrypoint,
            })
        }
        _ => None,
    };
    let roots = match seed(storage, &base_dirs(), host.as_ref()) {
        Ok(roots) => roots,
        Err(error) => {
            eprintln!("zcode-cli-rust: official plugin seed failed: {error:#}");
            vec![]
        }
    };
    seeded.push((storage.to_owned(), roots.clone()));
    roots
}

/// TS `candidateBaseDirs`：入口所在目录、进程 cwd；另加开发/测试用的显式候选。
fn base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("ZCODE_OFFICIAL_PLUGINS_BASE_DIR")
        && !dir.is_empty()
    {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
    {
        dirs.push(dir);
    }
    if let Ok(dir) = std::env::current_dir() {
        dirs.push(dir);
    }
    dirs
}

pub(crate) fn seed(storage: &Path, bases: &[PathBuf], host: Option<&Host>) -> Result<Vec<PathBuf>> {
    let assets = assets();
    let plugins: Vec<SeedPlugin> = assets
        .definitions
        .iter()
        .filter_map(|definition| {
            let root = find_root(definition, bases)?;
            let files = collect_files(&root, definition, assets).ok()?;
            let hash = hash_files(&files);
            let available: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
            let missing = definition
                .required
                .iter()
                .filter(|p| !available.contains(&p.as_str()))
                .cloned()
                .collect();
            Some(SeedPlugin {
                definition,
                files,
                hash,
                missing,
            })
        })
        .collect();
    if plugins.is_empty() {
        return Ok(vec![]);
    }
    super::official_plugins_marketplace::write(storage, &plugins)?;
    let failed = cache::seed_plugins(storage, &plugins, host)?;
    Ok(failed
        .into_iter()
        // CUA 的 frame contract 随 wrapper 原子升级，旧缓存不能回落（TS 同一规则）。
        .filter(|d| d.name != "computer-use")
        .filter_map(|d| cache::usable_fallback(storage, d))
        .collect())
}

fn find_root(definition: &Definition, bases: &[PathBuf]) -> Option<PathBuf> {
    bases.iter().find_map(|base| {
        definition.root_candidates.iter().find_map(|relative| {
            let root = super::lexical_path::normalize(&base.join(relative));
            root.join(".zcode-plugin")
                .join("plugin.json")
                .is_file()
                .then_some(root)
        })
    })
}

fn collect_files(root: &Path, definition: &Definition, assets: &Assets) -> Result<Vec<SeedFile>> {
    let allowed: Vec<&str> = assets
        .included
        .iter()
        .chain(&definition.runtime_top_level)
        .map(String::as_str)
        .collect();
    let mut files = Vec::new();
    walk(root, root, &allowed, 0, &mut files)?;
    files.sort_by(|a, b| locale_compare(&a.path, &b.path));
    Ok(files)
}

fn walk(
    root: &Path,
    dir: &Path,
    allowed: &[&str],
    depth: usize,
    out: &mut Vec<SeedFile>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // TS walkFiles：跳过规则作用于目录与文件同名项。
        if matches!(
            name.as_str(),
            ".turbo" | "coverage" | ".venv" | "__pycache__"
        ) || (name == "node_modules" && !(depth == 0 && allowed.contains(&"node_modules")))
        {
            continue;
        }
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            walk(root, &path, allowed, depth + 1, out)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let segments: Vec<&str> = relative.split('/').collect();
            if segments.contains(&".DS_Store") || segments.iter().any(|s| s.ends_with(".pyc")) {
                continue;
            }
            if !allowed.contains(&segments[0]) {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            let mode = mode_for(&relative, source_mode(&entry.metadata()?));
            out.push(SeedFile {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                path: relative,
                source: path,
                mode,
            });
        }
    }
    Ok(())
}

#[cfg(unix)]
fn source_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn source_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    // Node 在 Windows 上 stat 的 mode 不含执行位。
    None
}

/// TS `modeForSeedFile`。
pub(crate) fn mode_for(path: &str, source: Option<u32>) -> u32 {
    if source.is_some_and(|mode| mode & 0o111 != 0) {
        return 0o755;
    }
    let lower = path.to_ascii_lowercase();
    if lower == "dist/mcp/server.js" || lower.ends_with("/dist/mcp/server.js") {
        return 0o755;
    }
    if path.starts_with("hooks/")
        && ![".json", ".md", ".txt"]
            .iter()
            .any(|ext| lower.ends_with(ext))
    {
        return 0o755;
    }
    0o644
}

/// TS `hashSeedFiles`：`sha256(JSON.stringify([[path, sha256, mode], ...]))`。
fn hash_files(files: &[SeedFile]) -> String {
    let rows = Json::Array(
        files
            .iter()
            .map(|f| {
                Json::Array(vec![
                    Json::str(&f.path),
                    Json::str(&f.sha256),
                    Json::Number(f.mode.into()),
                ])
            })
            .collect(),
    );
    format!("{:x}", Sha256::digest(rows.compact().as_bytes()))
}

/// TS `String.prototype.localeCompare`（ICU 根排序在 ASCII 上的行为）：先比一级（忽略大小写，
/// 标点按生成表的顺序且不可忽略），再比三级（小写在前），最后按码元。
pub(crate) fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    let collation = &assets().collation;
    let primary = |c: char| {
        let lower = c.to_ascii_lowercase();
        collation
            .iter()
            .position(|x| *x == lower)
            .unwrap_or(collation.len() + lower as usize)
    };
    let key = |s: &str| s.chars().map(primary).collect::<Vec<_>>();
    let tertiary = |s: &str| {
        s.chars()
            .map(|c| c.is_ascii_uppercase())
            .collect::<Vec<_>>()
    };
    key(left)
        .cmp(&key(right))
        .then_with(|| tertiary(left).cmp(&tertiary(right)))
        .then_with(|| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_matches_ts_locale_compare() {
        let corpus =
            Json::parse(include_str!("../tests/fixtures/official_plugin_sort.json")).unwrap();
        let text = |key: &str| -> Vec<String> {
            corpus
                .get(key)
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        };
        let mut samples = text("samples");
        samples.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(samples, text("sorted"));
    }

    #[test]
    fn modes_follow_ts_rules() {
        assert_eq!(mode_for("dist/mcp/server.js", None), 0o755);
        assert_eq!(mode_for("hooks/pre.sh", None), 0o755);
        assert_eq!(mode_for("hooks/config.json", None), 0o644);
        assert_eq!(mode_for("skills/a/SKILL.md", Some(0o100755)), 0o755);
        assert_eq!(mode_for("skills/a/SKILL.md", Some(0o100644)), 0o644);
    }
}
