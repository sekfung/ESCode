//! git 域的文件侧读取（TS `gitCliHelpers.ts` 的 `buildUntrackedStats` / `buildUntrackedTextDiffResult` /
//! `fileExists` 与 `gitCliRepo.ts` 的 `readWorkingTreePreviewContent`）。

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use super::parse::{LineStat, NumstatMap, StatusEntry};
use super::paths::{self, js_trim, normalize_git_path, to_absolute_path};

/// TS `GIT_UNTRACKED_STAT_MAX_BYTES` / `_CHUNK_BYTES` / `_CONCURRENCY`。
const UNTRACKED_STAT_MAX_BYTES: u64 = 1024 * 1024;
const UNTRACKED_STAT_CHUNK_BYTES: usize = 64 * 1024;
const UNTRACKED_STAT_CONCURRENCY: usize = 4;
/// TS `DEFAULT_GIT_DIFF_BYTES`（预览上限）。
pub const DIFF_PREVIEW_BYTES: u64 = 1024 * 1024;

/// TS `countUntrackedFileLines`：非普通文件 / 超限 / 含 NUL → 0；否则换行数（末尾无换行 +1）。
async fn count_lines(path: &str) -> std::io::Result<i64> {
    let info = tokio::fs::metadata(path).await?;
    if !info.is_file() || info.len() > UNTRACKED_STAT_MAX_BYTES {
        return Ok(0);
    }
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; UNTRACKED_STAT_CHUNK_BYTES];
    let (mut total, mut newlines, mut last) = (0_u64, 0_i64, b'\n');
    loop {
        // 文件可能在 stat 后增长：实际读取同样受预算约束，多读一字节只用于识别越界。
        let budget = usize::try_from(UNTRACKED_STAT_MAX_BYTES + 1 - total).unwrap_or(usize::MAX);
        let length = buffer.len().min(budget);
        let read = file.read(&mut buffer[..length]).await?;
        if read == 0 {
            return Ok(newlines + i64::from(last != b'\n'));
        }
        total += read as u64;
        if total > UNTRACKED_STAT_MAX_BYTES {
            return Ok(0);
        }
        for byte in &buffer[..read] {
            if *byte == 0 {
                return Ok(0);
            }
            if *byte == b'\n' {
                newlines += 1;
            }
        }
        last = buffer[read - 1];
    }
}

/// TS `buildUntrackedStats`：未跟踪文件的新增行数（并发 4、读失败按 0）。
pub async fn untracked_stats(repo_root: &str, entries: &[StatusEntry]) -> NumstatMap {
    let paths: Vec<String> = entries
        .iter()
        .filter(|entry| entry.is_untracked)
        .map(|entry| entry.path.clone())
        .collect();
    let mut results: Vec<Option<i64>> = vec![None; paths.len()];
    for (chunk_start, chunk) in paths
        .chunks(UNTRACKED_STAT_CONCURRENCY)
        .enumerate()
        .map(|(index, chunk)| (index * UNTRACKED_STAT_CONCURRENCY, chunk))
    {
        let counts = join_all(
            chunk
                .iter()
                .map(|path| {
                    let absolute = to_absolute_path(repo_root, path);
                    async move { count_lines(&absolute).await.unwrap_or(0) }
                })
                .collect(),
        )
        .await;
        for (offset, count) in counts.into_iter().enumerate() {
            results[chunk_start + offset] = Some(count);
        }
    }
    let mut stats = NumstatMap::default();
    for (path, count) in paths.into_iter().zip(results) {
        stats.set(
            path,
            LineStat {
                added: count.unwrap_or(0),
                removed: 0,
                kind: None,
                original_path: None,
            },
        );
    }
    stats
}

/// 批内并发等待（TS 的 4 个 worker 并发读；结果顺序与入参一致）。
async fn join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pinned: Vec<std::pin::Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = (0..pinned.len()).map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut pending = false;
        for (index, future) in pinned.iter_mut().enumerate() {
            if outputs[index].is_some() {
                continue;
            }
            match future.as_mut().poll(context) {
                std::task::Poll::Ready(output) => outputs[index] = Some(output),
                std::task::Poll::Pending => pending = true,
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    outputs
        .into_iter()
        .map(|output| output.expect("ready"))
        .collect()
}

/// TS `fileExists`（`fs.access`：跟随符号链接，断链为 false）。
pub async fn exists(path: &str) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

/// TS `readWorkingTreePreviewContent`：普通文件、≤1 MiB、无 NUL 才返回文本。
pub async fn working_tree_preview(path: &str) -> Option<String> {
    let info = tokio::fs::metadata(path).await.ok()?;
    if !info.is_file() || info.len() > DIFF_PREVIEW_BYTES {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let content = String::from_utf8_lossy(&bytes).into_owned();
    (!content.contains('\0')).then_some(content)
}

fn diff_result(path: &str, availability: &str, summary: Option<&str>) -> Value {
    json!({
        "path": path,
        "availability": availability,
        "patch": null,
        "beforeContent": null,
        "afterContent": null,
        "summary": summary,
    })
}

/// TS `buildUntrackedTextDiffResult`：未跟踪文本文件的稳定 unified patch（读失败 → None）。
pub async fn untracked_text_diff(absolute: &str, repo_relative: &str) -> Option<Value> {
    let bytes = tokio::fs::read(absolute).await.ok()?;
    if bytes.contains(&0) {
        return Some(diff_result(
            absolute,
            "binary",
            Some("Binary diff is not previewable."),
        ));
    }
    if bytes.len() as u64 > DIFF_PREVIEW_BYTES {
        return Some(diff_result(
            absolute,
            "truncated",
            Some("Git diff output exceeded the preview limit."),
        ));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let normalized = text.replace("\r\n", "\n");
    let (lines, trailing) = if normalized.is_empty() {
        (Vec::new(), false)
    } else {
        let trailing = normalized.ends_with('\n');
        let mut lines: Vec<&str> = normalized.split('\n').collect();
        if trailing {
            lines.pop();
        }
        (lines, trailing)
    };
    let mut patch = vec![
        "--- /dev/null".to_owned(),
        format!("+++ b/{}", normalize_git_path(repo_relative)),
    ];
    if !lines.is_empty() {
        patch.push(format!("@@ -0,0 +1,{} @@", lines.len()));
        patch.extend(lines.iter().map(|line| format!("+{line}")));
        if !trailing {
            patch.push("\\ No newline at end of file".to_owned());
        }
    }
    Some(json!({
        "path": absolute,
        "availability": "patch",
        "patch": format!("{}\n", patch.join("\n")),
        "beforeContent": "",
        "afterContent": text,
        "summary": null,
    }))
}

/// TS `toUnavailableDiff`。
pub fn unavailable_diff(path: &str, summary: &str) -> Value {
    diff_result(path, "unavailable", Some(summary))
}

/// TS `toDiffResult`。
pub fn to_diff_result(
    path: &str,
    output: &super::command::GitOutput,
    allowed_exit_codes: &[i32],
    empty_summary: &str,
) -> Value {
    if output.timed_out {
        return diff_result(path, "unavailable", Some("Git diff command timed out."));
    }
    if output.output_truncated {
        return diff_result(
            path,
            "truncated",
            Some("Git diff output exceeded the preview limit."),
        );
    }
    if !output
        .exit_code
        .is_some_and(|code| allowed_exit_codes.contains(&code))
    {
        return diff_result(
            path,
            "unavailable",
            Some(&super::command::result_message(output)),
        );
    }
    if super::paths::js_trim(&output.stdout).is_empty() {
        return diff_result(path, "unavailable", Some(empty_summary));
    }
    if output.stdout.contains("GIT binary patch") || output.stdout.contains("Binary files ") {
        return diff_result(path, "binary", Some("Binary diff is not previewable."));
    }
    json!({
        "path": path,
        "availability": "patch",
        "patch": output.stdout,
        "beforeContent": null,
        "afterContent": null,
        "summary": null,
    })
}

/// TS `getWorkspaceRepositoryInfo` 的 `.git` 形态判定（任何异常 fail-open 为 main-tree）。
pub async fn linked_worktree_kind(repo_root: &str) -> &'static str {
    let entry = paths::resolve(repo_root, &[".git"]);
    let Ok(info) = tokio::fs::metadata(&entry).await else {
        return "main-tree";
    };
    if info.is_dir() || !info.is_file() {
        return "main-tree";
    }
    let Ok(content) = tokio::fs::read_to_string(&entry).await else {
        return "main-tree";
    };
    let normalized = content.replace("\r\n", "\n");
    let first = js_trim(normalized.split('\n').next().unwrap_or_default());
    let Some(raw) = first.strip_prefix("gitdir:") else {
        return "main-tree";
    };
    let raw = js_trim(raw);
    if raw.is_empty() {
        return "main-tree";
    }
    let resolved = if paths::is_absolute(raw) {
        raw.to_owned()
    } else {
        paths::resolve(repo_root, &[raw])
    };
    if resolved.replace('\\', "/").contains("/.git/worktrees/") {
        "linked-worktree"
    } else {
        "main-tree"
    }
}
