//! skill 同步候选扫描（TS `skillSyncService.ts` 的 `collectUserSkillCandidates` 及其依赖）。
//!
//! 扫描策略复用 `skills` 域（同一份 `should_walk` / 深度上限），元数据按 TS 的局部解析口径
//! （只认 `SKILL.md` 起始的 frontmatter；YAML 解析失败回退目录名），避免与 skills 域的宽松回退产生分歧。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};
use sha2::{Digest, Sha256};

use super::error::SkillSyncError;
use crate::git::paths::js_trim;
use crate::skills::{Compare, policy, walk_markdown_paths};

pub struct Candidate {
    pub id: String,
    pub name: String,
    pub directory_name: String,
    pub description: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

impl Candidate {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "directoryName": self.directory_name,
            "description": self.description,
            "path": self.path.to_string_lossy(),
            "sizeBytes": self.size_bytes,
        })
    }
}

pub fn user_escode_root(home: &Path) -> PathBuf {
    home.join(".escode").join("skills")
}

pub fn user_agents_root(home: &Path) -> PathBuf {
    home.join(".agents").join("skills")
}

/// TS `collectUserSkillCandidates`：`.escode` 优先，`.agents` 排除被覆盖的目录名与重名 skill。
pub fn collect(home: &Path, compare: &Compare) -> Result<Vec<Candidate>, SkillSyncError> {
    let escode_root = user_escode_root(home);
    let agents_root = user_agents_root(home);
    let raw_escode = collect_in_root(&escode_root, compare)?;
    let covered = covered_directory_names(&escode_root)?;
    let escode_names: BTreeSet<String> = raw_escode
        .iter()
        .map(|candidate| name_key(&candidate.name))
        .collect();
    let mut seen = BTreeSet::new();
    let mut candidates = dedupe_by_canonical_path(raw_escode, &mut seen);
    let agents: Vec<Candidate> = collect_in_root(&agents_root, compare)?
        .into_iter()
        .filter(|candidate| {
            !covered.contains(&candidate.directory_name)
                && !escode_names.contains(&name_key(&candidate.name))
        })
        .collect();
    candidates.extend(dedupe_by_canonical_path(agents, &mut seen));
    candidates.sort_by(|left, right| compare(&left.directory_name, &right.directory_name));
    Ok(candidates)
}

fn dedupe_by_canonical_path(
    candidates: Vec<Candidate>,
    seen: &mut BTreeSet<PathBuf>,
) -> Vec<Candidate> {
    candidates
        .into_iter()
        .filter(|candidate| {
            let canonical =
                std::fs::canonicalize(&candidate.path).unwrap_or_else(|_| candidate.path.clone());
            seen.insert(canonical)
        })
        .collect()
}

fn collect_in_root(root: &Path, compare: &Compare) -> Result<Vec<Candidate>, SkillSyncError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    for skill_path in walk_markdown_paths(root) {
        let Some(directory) = skill_path.parent().map(std::path::Path::to_path_buf) else {
            continue;
        };
        let Ok(relative) = directory.strip_prefix(root) else {
            continue;
        };
        let directory_name = relative.to_string_lossy().replace('\\', "/");
        if directory_name.is_empty() {
            continue;
        }
        let Some(content) = read_skill_file(&skill_path)? else {
            continue;
        };
        let fallback = directory_name
            .rsplit('/')
            .next()
            .unwrap_or(&directory_name)
            .to_owned();
        let (name, description) = parse_metadata(&content, &fallback);
        candidates.push(Candidate {
            id: candidate_id(&directory),
            name,
            directory_name,
            description,
            path: skill_path,
            size_bytes: recursive_size(&directory, &mut BTreeSet::new())?,
        });
    }
    candidates.sort_by(|left, right| compare(&left.directory_name, &right.directory_name));
    Ok(candidates)
}

/// TS `collectCoveredSkillDirectoryNamesInRoot`：`.escode` 下所有含 `SKILL.md` 的目录（含非候选路径）。
fn covered_directory_names(root: &Path) -> Result<BTreeSet<String>, SkillSyncError> {
    let mut names = BTreeSet::new();
    if !root.exists() {
        return Ok(names);
    }
    let mut stack = vec![(root.to_path_buf(), String::new(), 0_usize)];
    while let Some((directory, relative, depth)) = stack.pop() {
        if !relative.is_empty()
            && read_skill_file(&directory.join(policy::SKILL_FILE_NAME))?.is_some()
        {
            names.insert(relative.clone());
        }
        if depth >= policy::MAX_SCAN_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !policy::should_walk(&name) {
                continue;
            }
            let path = entry.path();
            let is_dir = match entry.file_type() {
                Ok(kind) if kind.is_dir() => true,
                Ok(kind) if kind.is_symlink() => {
                    std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir())
                }
                _ => false,
            };
            if is_dir {
                stack.push((path, join_relative(&relative, &name), depth + 1));
            }
        }
    }
    Ok(names)
}

/// 用户级两个根（`.escode` 先、`.agents` 后）：第一个出现的 skill 名对应目录。
pub fn directory_path_by_name(home: &Path) -> Result<BTreeMap<String, PathBuf>, SkillSyncError> {
    let mut result = collect_directory_path_by_name(&user_escode_root(home))?;
    for (key, path) in collect_directory_path_by_name(&user_agents_root(home))? {
        result.entry(key).or_insert(path);
    }
    Ok(result)
}

fn collect_directory_path_by_name(
    root: &Path,
) -> Result<BTreeMap<String, PathBuf>, SkillSyncError> {
    let mut result = BTreeMap::new();
    if !root.exists() {
        return Ok(result);
    }
    for skill_path in walk_markdown_paths(root) {
        let Some(content) = read_skill_file(&skill_path)? else {
            continue;
        };
        let Some(directory) = skill_path.parent() else {
            continue;
        };
        let fallback = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (name, _) = parse_metadata(&content, &fallback);
        result
            .entry(name_key(&name))
            .or_insert_with(|| directory.to_path_buf());
    }
    Ok(result)
}

pub fn name_key(name: &str) -> String {
    js_trim(name).to_lowercase()
}

pub(super) fn read_skill_file(path: &Path) -> Result<Option<String>, SkillSyncError> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// TS `parseSkillMetadata`：frontmatter 的 `name`/`description`；解析失败回退目录名/空描述。
pub(super) fn parse_metadata(content: &str, fallback_name: &str) -> (String, String) {
    let normalized = content.replace("\r\n", "\n");
    let Some(frontmatter) = split_frontmatter(&normalized) else {
        return (fallback_name.to_owned(), String::new());
    };
    let Ok(documents) = YamlOwned::load_from_str(frontmatter) else {
        return (fallback_name.to_owned(), String::new());
    };
    if documents.len() != 1 {
        return (fallback_name.to_owned(), String::new());
    }
    let YamlOwned::Mapping(map) = &documents[0] else {
        return (fallback_name.to_owned(), String::new());
    };
    let (mut name, mut description) = (None, None);
    for (key, value) in map {
        let YamlOwned::Value(ScalarOwned::String(key)) = key else {
            continue;
        };
        let text = match value {
            YamlOwned::Value(ScalarOwned::String(text)) => Some(text),
            _ => None,
        };
        match key.as_str() {
            "name" => name = text.map(|text| js_trim(text).to_owned()),
            "description" => description = text.map(|text| js_trim(text).to_owned()),
            _ => {}
        }
    }
    let name = name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback_name.to_owned());
    (name, description.unwrap_or_default())
}

/// `/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/`（输入已归一 CRLF）。
fn split_frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let mut from = 0;
    while let Some(found) = rest[from..].find("\n---") {
        let at = from + found;
        match rest.as_bytes().get(at + 4) {
            None | Some(b'\n') => return Some(&rest[..at]),
            _ => from = at + 1,
        }
    }
    None
}

fn candidate_id(directory: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(directory.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// TS `computeRecursiveSize`：软链跟随目标，目录按 realpath 去重（环返回 0）。
fn recursive_size(path: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<u64, SkillSyncError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        let Ok(target) = std::fs::metadata(path) else {
            return Ok(0);
        };
        if target.is_file() {
            return Ok(target.len());
        }
        return if target.is_dir() {
            directory_size(path, visited)
        } else {
            Ok(0)
        };
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if metadata.is_dir() {
        return directory_size(path, visited);
    }
    Ok(0)
}

fn directory_size(path: &Path, visited: &mut BTreeSet<PathBuf>) -> Result<u64, SkillSyncError> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical) {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in std::fs::read_dir(path)? {
        total += recursive_size(&entry?.path(), visited)?;
    }
    Ok(total)
}

fn join_relative(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;
