//! 技能目录扫描缓存（docs/specs/rust-u4-directory-cache.md）。
//!
//! 键是扫描根目录，值是「目录指纹 + 解析后的技能条目」。指纹只覆盖本次 walk 得到的
//! `SKILL.md` 集合及其同级 `_meta.json` 的 `(len, mtime_ms)`；任一变化都会重新读取解析。
//! scope/plugin 字段与 `covered_by_escode` 过滤不进入缓存，由 discover 按当前 workspace 物化。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::frontmatter;
use super::walk::walk;
use crate::git::paths::js_trim;

const MAX_DESCRIPTION_LENGTH: usize = 1024;
const MAX_CACHE_ENTRIES: usize = 256;

#[derive(Clone)]
pub(super) struct CachedSkill {
    pub source_path: PathBuf,
    pub canonical_path: PathBuf,
    pub name: String,
    pub description: String,
    pub body: String,
    pub metadata: Option<Value>,
}

pub(super) struct CachedScan {
    pub skills: Vec<CachedSkill>,
    /// 读取/解析阶段产生的诊断（walk 诊断由调用方在每次扫描时重新生成）。
    pub diagnostics: Vec<Value>,
}

struct CacheEntry {
    fingerprint: String,
    scan: Arc<CachedScan>,
}

static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn norm_scan_root(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

fn file_stamp(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or_default();
            format!("{}:{}", meta.len(), mtime_ms)
        }
        Err(_) => "-".to_owned(),
    }
}

/// 目录指纹：排序后的技能文件路径 + 每个文件/同级 `_meta.json` 的 `(len, mtime_ms)`。
fn fingerprint(paths: &BTreeSet<PathBuf>) -> String {
    let mut hasher = Sha256::new();
    for path in paths {
        hasher.update(norm_scan_root(path).as_bytes());
        hasher.update(b"\0");
        hasher.update(file_stamp(path).as_bytes());
        hasher.update(b"\0");
        if let Some(parent) = path.parent() {
            let meta_path = parent.join("_meta.json");
            if exists(&meta_path) {
                hasher.update(norm_scan_root(&meta_path).as_bytes());
                hasher.update(b"\0");
                hasher.update(file_stamp(&meta_path).as_bytes());
                hasher.update(b"\0");
            }
        }
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_metadata(skill: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(skill.parent()?.join("_meta.json"))
        .ok()
        .filter(|text| !text.is_empty())?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    let parsed = parsed.as_object()?;
    let mut meta = Map::new();
    for key in ["slug", "version", "ownerId"] {
        if let Some(text) = parsed
            .get(key)
            .and_then(Value::as_str)
            .map(js_trim)
            .filter(|text| !text.is_empty())
        {
            meta.insert(key.to_owned(), json!(text));
        }
    }
    if let Some(at) = parsed
        .get("publishedAt")
        .filter(|value| value.as_f64().is_some_and(f64::is_finite))
    {
        meta.insert("publishedAt".to_owned(), at.clone());
    }
    (!meta.is_empty()).then_some(Value::Object(meta))
}

fn parse_skill(skill_path: &Path, diagnostics: &mut Vec<Value>) -> Option<CachedSkill> {
    if !exists(skill_path) {
        return None;
    }
    let markdown = match std::fs::read(skill_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            diagnostics.push(json!({"code": "skill_read_failed", "severity": "warning",
                "message": error.to_string(), "path": skill_path.to_string_lossy()}));
            return None;
        }
    };
    let parsed = frontmatter::read(&markdown);
    let folder = skill_path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let raw_name = if parsed.has_frontmatter {
        js_trim(&parsed.name).to_owned()
    } else {
        folder.clone()
    };
    let name = if raw_name.is_empty() {
        folder
    } else {
        raw_name
    };
    if name.is_empty() {
        diagnostics.push(json!({"code": "skill_missing_name", "severity": "error",
            "message": format!("Skill frontmatter must include a name: {}", skill_path.display()), "path": skill_path.to_string_lossy()}));
        return None;
    }
    let description = if parsed.has_frontmatter {
        js_trim(&parsed.description).to_owned()
    } else {
        String::new()
    };
    if description.encode_utf16().count() > MAX_DESCRIPTION_LENGTH {
        diagnostics.push(
            json!({"code": "skill_description_too_long", "severity": "error",
            "message": format!("Skill description is too long (>{MAX_DESCRIPTION_LENGTH}): {name}"),
            "path": skill_path.to_string_lossy(), "skillName": name}),
        );
        return None;
    }
    let canonical_path =
        std::fs::canonicalize(skill_path).unwrap_or_else(|_| skill_path.to_path_buf());
    Some(CachedSkill {
        source_path: skill_path.to_path_buf(),
        canonical_path,
        name,
        description,
        body: js_trim(&parsed.body).to_owned(),
        metadata: read_metadata(skill_path),
    })
}

/// 扫描一个目录：命中且指纹一致时复用上次解析结果，否则重新读取并解析。
pub(super) fn scan_root_cached(
    root: &Path,
    compare: &super::discover::Compare,
    diagnostics: &mut Vec<Value>,
) -> Arc<CachedScan> {
    let paths = walk(root, diagnostics);
    let fingerprint = fingerprint(&paths);
    let key = norm_scan_root(root);

    {
        let guard = cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = guard
            .get(&key)
            .filter(|entry| entry.fingerprint == fingerprint)
        {
            diagnostics.extend(entry.scan.diagnostics.iter().cloned());
            return Arc::clone(&entry.scan);
        }
    }

    let mut ordered: Vec<&PathBuf> = paths.iter().collect();
    ordered.sort_by(|left, right| compare(&left.to_string_lossy(), &right.to_string_lossy()));
    let mut parse_diagnostics = Vec::new();
    let skills = ordered
        .into_iter()
        .filter_map(|path| parse_skill(path, &mut parse_diagnostics))
        .collect::<Vec<_>>();
    let scan = Arc::new(CachedScan {
        skills,
        diagnostics: parse_diagnostics.clone(),
    });
    diagnostics.extend(parse_diagnostics);

    let mut guard = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.len() >= MAX_CACHE_ENTRIES {
        guard.clear();
    }
    guard.insert(
        key,
        CacheEntry {
            fingerprint,
            scan: Arc::clone(&scan),
        },
    );
    scan
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
