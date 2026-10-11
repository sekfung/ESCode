//! Bash 对读取状态的影响（docs/specs/rust-bash-model-content.md），对齐 TS `bash-read-file-state.ts`：
//! 格式化/修复类命令改写已读文件时提示重新 Read；cat/head/tail/sed -n/grep 读过的文件记为已读，
//! 之后可直接 Edit/Write（TS 回填 readFileState）。
use super::tool_files::FileState;
use crate::contract::ToolOutput;
use crate::domain::{bash_model_content, bash_read_sources as sources};
use std::{path::Path, time::SystemTime};
use tokio::sync::Mutex;

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_HINT_PATHS: usize = 5;

/// 前台 Bash 结果之后调用；`started` 为命令开始时间。改动了结果时重新生成模型正文。
pub(super) async fn apply(state: &Mutex<FileState>, workspace: &Path, output: &mut ToolOutput, command: &str, started: SystemTime) {
    let data = &output.data;
    // TS shouldSkipBashReadFileStateEffects：后台、图片、中断与提供方错误都不影响读取状态。
    if data.get("backgroundTaskId").is_some()
        || data["isImage"] == true
        || data["interrupted"] == true
        || bash_model_content::is_provider_error(data)
    {
        return;
    }
    if sources::is_write_command(command)
        && let Some(hint) = stale_hint(state, workspace, started).await
    {
        output.data["staleReadFileStateHint"] = hint.into();
        *output = super::tool_process::shell_output(std::mem::take(&mut output.data));
    }
    backfill(state, workspace, &output.data, command).await;
}

/// 命令开始后被改动过的已读文件（TS 另要求晚于读取时的 mtime；读取总在命令之前，二者等价）。
async fn stale_hint(state: &Mutex<FileState>, workspace: &Path, started: SystemTime) -> Option<String> {
    let paths = state.lock().await.paths();
    let mut changed = vec![];
    for path in paths {
        let modified = tokio::fs::metadata(&path).await.ok().and_then(|m| m.modified().ok());
        if modified.is_some_and(|m| m > started) {
            changed.push(path);
        }
    }
    if changed.is_empty() {
        return None;
    }
    let root = escode_cli_host::realpath(workspace).await.unwrap_or_else(|_| workspace.to_owned());
    let shown: Vec<String> = changed
        .iter()
        .take(MAX_HINT_PATHS)
        .map(|p| p.strip_prefix(&root).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string_lossy().into_owned()))
        .collect();
    let hidden = changed.len().saturating_sub(MAX_HINT_PATHS);
    let more = if hidden > 0 { format!(" and {hidden} more") } else { String::new() };
    let word = if changed.len() == 1 { "file" } else { "files" };
    Some(format!(
        "[This command modified {} {word} you've previously read: {}{more}. Call Read before editing.]",
        changed.len(),
        shown.join(", ")
    ))
}

async fn backfill(state: &Mutex<FileState>, workspace: &Path, data: &serde_json::Value, command: &str) {
    // 截断的 stdout 不代表模型看到了完整内容，不能绕过读后再写的约束。
    if data["stdoutTruncated"] == true {
        return;
    }
    let exit_zero = data["exitCode"].as_i64() == Some(0);
    for source in sources::collect(command).into_iter().filter(|s| !s.requires_exit_zero || exit_zero) {
        let requested = workspace.join(&source.file_path);
        let Ok(key) = escode_cli_host::realpath(&requested).await else {
            continue;
        };
        if state.lock().await.contains(&key) {
            continue;
        }
        let Ok(meta) = tokio::fs::metadata(&key).await else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&key).await else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Some(selected) = sources::select(text, &source) else {
            continue;
        };
        // TS 回填项 isPartialView 为 false：之后 Edit 与 Write 都可直接进行；同范围的 Read 返回未变提示。
        let stamp = super::file_state::Stamp::of(&key).await;
        let mut state = state.lock().await;
        state.observe(key.clone(), &bytes, true);
        if let Some(stamp) = stamp {
            let limit = selected.limit.map(|l| l as u64);
            state.record_range(key, selected.offset.unwrap_or(1) as u64, limit, stamp);
        }
    }
}
