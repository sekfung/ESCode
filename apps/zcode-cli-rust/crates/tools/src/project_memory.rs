//! 项目记忆的文件系统部分（docs/specs/rust-project-memory.md）：按配置解析记忆根、创建目录、读取索引与 manifest。
use super::extension_config as config;
use crate::domain::memory::{self, ManifestEntry};
use std::path::{Path, PathBuf};

/// TS resolveEnabledProjectMemoryRoot + loadProjectMemoryRoot + loadProjectMemoryIndexContent。
pub(crate) async fn resolve(
    cwd: &Path,
    workspace: &Path,
) -> Option<crate::contract::ProjectMemory> {
    let settings = config::load(cwd).await.unwrap_or_default();
    if settings["features"]["memory"] == false || settings["memory"]["use"] == false {
        return None;
    }
    // 与插件存储同源：storage() 为 <base>/cli/plugins，记忆位于 <base>/cli/memories。
    let cli_root = config::storage(&settings).parent()?.to_owned();
    let workspace = std::path::absolute(workspace).ok()?;
    let path = workspace.to_string_lossy().into_owned();
    // TS：无 identity 时以绝对路径为 key，Windows 下转小写。
    let key = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.clone()
    };
    let name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let identity = std::env::var("ZCODE_WORKSPACE_IDENTITY").ok();
    let root = cli_root
        .join("memories")
        .join("projects")
        .join(memory::project_directory(identity.as_deref(), &key, &name))
        .join("memory");
    // 目录预创建失败不阻断；实际 Write/Edit 仍返回原始错误。
    let _ = tokio::fs::create_dir_all(&root).await;
    let index_path = root.join("MEMORY.md");
    let index = tokio::fs::read(&index_path)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    Some(crate::contract::ProjectMemory {
        root: root.to_string_lossy().into_owned(),
        index_path: index_path.to_string_lossy().into_owned(),
        index,
    })
}

/// TS scanMemoryManifest：递归收集 `.md`（跟随文件 symlink），排除 MEMORY.md，按 mtime 倒序取前 200。
pub(crate) async fn manifest(root: &str) -> Vec<ManifestEntry> {
    let root = PathBuf::from(root);
    let mut files = vec![];
    collect(&root, &mut files).await;
    let mut entries = vec![];
    for path in files {
        let (Ok(meta), Ok(bytes)) = (
            tokio::fs::metadata(&path).await,
            tokio::fs::read(&path).await,
        ) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let preview = text
            .split('\n')
            .take(memory::MANIFEST_PREVIEW_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        let (description, kind) = memory::manifest_frontmatter(&preview);
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let filename = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        entries.push(ManifestEntry {
            description,
            filename,
            mtime_ms,
            kind,
        });
    }
    entries.sort_by(|a, b| b.mtime_ms.total_cmp(&a.mtime_ms));
    entries.truncate(memory::MANIFEST_FILE_LIMIT);
    entries
}
async fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(kind) = entry.file_type().await else {
            continue;
        };
        let candidate = path.extension().is_some_and(|e| e == "md")
            && path.file_name().is_some_and(|n| n != "MEMORY.md");
        if kind.is_dir() {
            Box::pin(collect(&path, out)).await;
        } else if candidate
            && (kind.is_file()
                // 单个失效的文件 symlink 不影响其他 manifest 项（TS collectMemoryPaths）。
                || (kind.is_symlink()
                    && tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_file())))
        {
            out.push(path);
        }
    }
}

type Reads = tokio::sync::Mutex<
    std::collections::HashMap<
        String,
        std::sync::Arc<tokio::sync::Mutex<super::tool_files::FileState>>,
    >,
>;
/// 会话记忆上下文（记忆根, 来源会话）；`inherit` 复制该会话的读取状态（TS 记忆 agent 继承主会话
/// readFileState），`seed` 记为已完整读取。
pub(crate) async fn set_context(
    memory: &tokio::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
    reads: &Reads,
    (session, root, origin, inherit, seed): (&str, &str, &str, Option<&str>, Option<&str>),
) {
    memory
        .lock()
        .await
        .insert(session.into(), (root.into(), origin.into()));
    let mut reads = reads.lock().await;
    if let Some(from) = inherit {
        let copied = match reads.get(from) {
            Some(state) => state.lock().await.clone(),
            None => Default::default(),
        };
        reads.insert(
            session.into(),
            std::sync::Arc::new(tokio::sync::Mutex::new(copied)),
        );
    }
    let Some(path) = seed else {
        return;
    };
    let state = reads.entry(session.into()).or_default().clone();
    drop(reads);
    if let Ok(real) = zcode_cli_host::realpath(Path::new(path)).await
        && let Ok(bytes) = tokio::fs::read(&real).await
    {
        // TS isPartialView：格式化后的索引与原文不同（去 frontmatter/截断/首尾空白）即为部分视图。
        let text = String::from_utf8_lossy(&bytes);
        let full = memory::format_project_index(&text) == text;
        state.lock().await.observe(real, &bytes, full);
    }
}
