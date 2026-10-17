//! Claude Code 原生历史扫描与导入（S10，TS `session/claude-native/*`）。
//!
//! 扫描边界：`~/.claude/projects`（homedir / HOME / dataBaseDir 三处去重），跳过
//! subagents/worktree(s)；导入时复制 jsonl 到 `<dataBaseDir>/.escode/v2/agent-config/claude/...`，
//! 由 host-rpc 再创建真实 ESCode session 并写索引。

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod head;
mod parser;
mod timestamp;

pub use parser::ImportedSession;

const IGNORED_DIRS: [&str; 3] = ["subagents", "worktree", "worktrees"];

#[derive(Debug, Clone)]
pub struct Candidate {
    pub session_id: String,
    pub workspace_path: String,
    pub source_path: String,
    pub updated_at: i64,
    pub created_at: Option<i64>,
    pub preview_title: Option<String>,
}

impl Candidate {
    pub fn to_json(&self) -> Value {
        let mut candidate = json!({
            "provider": "claude",
            "sessionId": self.session_id,
            "workspacePath": self.workspace_path,
            "sourcePath": self.source_path,
            "updatedAt": self.updated_at,
        });
        if let Some(created_at) = self.created_at {
            candidate["createdAt"] = Value::from(created_at);
        }
        if let Some(title) = self.preview_title.as_ref() {
            candidate["previewTitle"] = Value::from(title.clone());
        }
        candidate
    }
}

fn roots(home: &Path, data_base_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut homes = BTreeSet::new();
    homes.insert(home.to_path_buf());
    if let Some(data_base_dir) = data_base_dir {
        homes.insert(data_base_dir.to_path_buf());
    }
    homes
        .into_iter()
        .map(|home| home.join(".claude").join("projects"))
        .collect()
}

fn ignored_dir(name: &str) -> bool {
    IGNORED_DIRS
        .iter()
        .any(|ignored| name.eq_ignore_ascii_case(ignored))
}

fn collect_jsonl(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && !ignored_dir(&name) => collect_jsonl(&path, files),
            Ok(kind) if kind.is_file() && name.ends_with(".jsonl") => files.push(path),
            _ => {}
        }
    }
}

fn find_session_file(root: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                if !ignored_dir(&name)
                    && let Some(found) = find_session_file(&path, session_id)
                {
                    return Some(found);
                }
            }
            Ok(kind) if kind.is_file() && name == file_name => return Some(path),
            _ => {}
        }
    }
    None
}

/// TS `normalize(resolve(path))`：绝对化并折叠 `.`/`..`；Windows 小写比较。
pub fn normalize_path_for_comparison(path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let text = normalized.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

fn is_worktree_workspace(path: &str) -> bool {
    let normalized = normalize_path_for_comparison(path).replace('\\', "/");
    let segments: Vec<String> = normalized.split('/').map(str::to_lowercase).collect();
    segments.iter().enumerate().any(|(index, segment)| {
        segment == ".claude"
            && matches!(
                segments.get(index + 1).map(String::as_str),
                Some("worktree" | "worktrees")
            )
    })
}

fn mtime_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn candidate_of(path: &Path) -> Option<Candidate> {
    let metadata = std::fs::metadata(path).ok()?;
    let updated_at = mtime_ms(&metadata);
    let entries = parser::read_head(path, head::HEAD_LINES).ok()?;
    if head::has_sidechain_marker(&entries) {
        return None;
    }
    let info = head::extract_head_info(&entries);
    let workspace_path = info.workspace_path?;
    if is_worktree_workspace(&workspace_path) {
        return None;
    }
    let session_id = path.file_stem()?.to_string_lossy().into_owned();
    Some(Candidate {
        session_id,
        workspace_path,
        source_path: path.to_string_lossy().into_owned(),
        updated_at,
        created_at: info.created_at,
        preview_title: info.preview_title,
    })
}

pub fn scan_importable_sessions(
    home: &Path,
    data_base_dir: Option<&Path>,
    workspace_path: Option<&str>,
    modified_since: Option<i64>,
    limit: Option<usize>,
) -> Vec<Candidate> {
    let workspace_key = workspace_path.map(normalize_path_for_comparison);
    let mut collected = Vec::new();
    for root in roots(home, data_base_dir) {
        collect_jsonl(&root, &mut collected);
    }
    let files: BTreeSet<PathBuf> = collected.into_iter().collect();
    let mut candidates = Vec::new();
    for file in files {
        let Some(candidate) = candidate_of(&file) else {
            continue;
        };
        if modified_since.is_some_and(|since| candidate.updated_at < since) {
            continue;
        }
        if workspace_key
            .as_ref()
            .is_some_and(|key| normalize_path_for_comparison(&candidate.workspace_path) != *key)
        {
            continue;
        }
        candidates.push(candidate);
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.updated_at));
    if let Some(limit) = limit.filter(|limit| *limit > 0) {
        candidates.truncate(limit);
    }
    candidates
}

pub fn find_importable_session(
    home: &Path,
    data_base_dir: Option<&Path>,
    workspace_path: Option<&str>,
    session_id: &str,
) -> Option<Candidate> {
    let workspace_key = workspace_path.map(normalize_path_for_comparison);
    for root in roots(home, data_base_dir) {
        let Some(path) = find_session_file(&root, session_id) else {
            continue;
        };
        let Some(candidate) = candidate_of(&path) else {
            continue;
        };
        if workspace_key
            .as_ref()
            .is_some_and(|key| normalize_path_for_comparison(&candidate.workspace_path) != *key)
        {
            continue;
        }
        return Some(candidate);
    }
    None
}

/// TS `buildImportedClaudeTaskId`：`claude-import-` + sha256(`claude:{workspace}:{session}`) 前 24。
pub fn build_task_id(workspace_path: &str, session_id: &str) -> String {
    let digest = Sha256::digest(format!("claude:{workspace_path}:{session_id}").as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("claude-import-{}", &hex[..24])
}

fn workspace_hash(workspace_path: &str, identity: Option<&str>) -> String {
    let key = identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(workspace_path);
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    hex[..12].to_owned()
}

fn relative_projects_path(source_path: &Path) -> Result<PathBuf, String> {
    let components: Vec<_> = source_path.components().collect();
    let Some(index) = components
        .iter()
        .rposition(|component| component.as_os_str() == "projects")
    else {
        return Err(format!(
            "[claude-native] Claude 原生 session 路径非法: {}",
            source_path.display()
        ));
    };
    let mut relative = PathBuf::new();
    for component in &components[index + 1..] {
        relative.push(component.as_os_str());
    }
    Ok(relative)
}

pub struct CopiedSession {
    pub output_path: PathBuf,
    pub created_output_paths: Vec<PathBuf>,
}

pub fn copy_session_file_to_workspace(
    data_base_dir: &Path,
    workspace_path: &str,
    identity: Option<&str>,
    source_path: &Path,
) -> Result<CopiedSession, String> {
    let relative = relative_projects_path(source_path)?;
    let output_path = data_base_dir
        .join(".escode")
        .join("v2")
        .join("agent-config")
        .join("claude")
        .join(workspace_hash(workspace_path, identity))
        .join("projects")
        .join(relative);
    let existed_before = output_path.exists();
    let parent = output_path
        .parent()
        .ok_or_else(|| "import output directory unavailable".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("mkdir {}: {error}", parent.display()))?;
    let temp = output_path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    std::fs::copy(source_path, &temp).map_err(|error| format!("copy imported session: {error}"))?;
    std::fs::rename(&temp, &output_path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("rename imported session: {error}")
    })?;
    Ok(CopiedSession {
        output_path: output_path.clone(),
        created_output_paths: if existed_before {
            Vec::new()
        } else {
            vec![output_path]
        },
    })
}

pub fn parse_session(
    source_path: &Path,
    workspace_path: &str,
    session_id: &str,
    fallback_created_at: Option<i64>,
    fallback_updated_at: Option<i64>,
) -> Result<ImportedSession, String> {
    parser::parse_session_file(
        source_path,
        workspace_path,
        session_id,
        &source_path.to_string_lossy(),
        fallback_created_at,
        fallback_updated_at,
    )
}

/// TS `getLegacyTaskSessionSnapshotPath`：`<data>/.escode/v2/sessions/<hash>/<taskId>.json`。
pub fn legacy_snapshot_path(
    data_base_dir: &Path,
    workspace_path: &str,
    identity: Option<&str>,
    task_id: &str,
) -> PathBuf {
    data_base_dir
        .join(".escode")
        .join("v2")
        .join("sessions")
        .join(workspace_hash(workspace_path, identity))
        .join(format!("{task_id}.json"))
}

pub fn write_legacy_snapshot(
    data_base_dir: &Path,
    workspace_path: &str,
    identity: Option<&str>,
    task_id: &str,
    meta: &Value,
    messages: &[Value],
) -> Result<PathBuf, String> {
    let path = legacy_snapshot_path(data_base_dir, workspace_path, identity, task_id);
    let body = serde_json::to_vec_pretty(&json!({"meta": meta, "messages": messages}))
        .map_err(|error| format!("serialize imported snapshot: {error}"))?;
    let mut bytes = body;
    bytes.push(b'\n');
    escode_host_core::atomic_write::write(&path, &bytes)
        .map_err(|error| format!("write imported snapshot: {error}"))?;
    Ok(path)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
