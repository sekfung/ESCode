use super::edit_apply;
use super::tools::{boolean, check_cancel, keys, resolve, string, uint};
use crate::contract::ToolOutput;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::{io::AsyncReadExt, sync::Mutex};
use tokio_util::sync::CancellationToken;
const READ_BYTES: usize = 64 * 1024;
const EDIT_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Default, Clone)]
pub struct FileState {
    entries: HashMap<PathBuf, Observation>,
}
impl FileState {
    /// 记为已读取（MEMORY.md 已注入上下文，TS 在加载索引时写入 readFileState）。
    pub(super) fn observe(&mut self, path: PathBuf, bytes: &[u8], full: bool) {
        let hash = Sha256::digest(bytes).to_vec();
        self.entries.insert(path, Observation { hash, full });
    }
}
#[derive(Clone)]
struct Observation {
    hash: Vec<u8>,
    full: bool,
}
pub struct FileTools<'a> {
    /// 启用记忆时的（记忆根, 来源会话）：写入记忆 Markdown 时补写 originSessionId。
    pub memory: Option<(&'a str, &'a str)>,
    /// 本轮模型的 inputFormat（PDF/图片能力）。
    pub input_format: Value,
    pub sink: Option<&'a crate::contract::EventSink>,
    pub checkpoint_root: &'a Path,
    pub cwd: &'a Path,
    pub artifacts: &'a Path,
    pub state: &'a Mutex<FileState>,
    pub writes: &'a Mutex<()>,
}
impl FileTools<'_> {
    pub async fn call(
        &self,
        name: &str,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let path = resolve(self.cwd, string(args, "file_path")?)?;
        if name == "Read" {
            let pdf = super::read_pdf::supports(&self.input_format, "supportsPdf");
            keys(
                args,
                if pdf {
                    &["file_path", "offset", "limit", "pages"]
                } else {
                    &["file_path", "offset", "limit"]
                },
            )?;
            return self.read(&path, args, cancel).await;
        }
        keys(
            args,
            if name == "Write" {
                &["file_path", "content"]
            } else {
                &["file_path", "old_string", "new_string", "replace_all"]
            },
        )?;
        let _guard = tokio::select! { _=cancel.cancelled()=>bail!("Cancelled"), lock=self.writes.lock()=>lock };
        self.write(name, &path, args, cancel).await
    }
    async fn remember(&self, path: PathBuf, hash: Vec<u8>, full: bool) {
        let mut state = self.state.lock().await;
        // 读取观察缓存有界；淘汰只会要求重新 Read，不会跳过新鲜度检查。
        if state.entries.len() >= 1024 && !state.entries.contains_key(&path) {
            state.entries.clear();
        }
        state.entries.insert(path, Observation { hash, full });
    }
    async fn read(
        &self,
        path: &Path,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let path = zcode_cli_host::realpath(path).await?;
        // TS 先按扩展名分派媒体（docs/specs/rust-media-read.md）。
        if let Some(mime) = super::read_image::mime_from_path(&path) {
            return super::read_image::read(&path, mime, cancel).await;
        }
        if let Some(mime) = super::read_image::video_mime_from_path(&path) {
            return super::read_image::read_video(&path, mime, cancel).await;
        }
        if super::read_pdf::supports(&self.input_format, "supportsPdf")
            && path.to_string_lossy().to_lowercase().ends_with(".pdf")
        {
            let pages = args["pages"].as_str();
            let image = super::read_pdf::supports(&self.input_format, "supportsImage");
            return super::read_pdf::read(&path, pages, image, cancel).await;
        }
        if !tokio::fs::metadata(&path).await?.is_file() {
            bail!("Read requires a regular file");
        }
        let mut file = tokio::fs::File::open(&path).await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() {
            bail!("Read requires a regular file");
        }
        let start = uint(args, "offset", 1)?.max(1);
        let limit = uint(args, "limit", 2000)?;
        if limit == 0 {
            bail!("limit must be positive");
        }
        let end = start.saturating_add(limit);
        let mut buf = [0u8; 8192];
        let mut selected = Vec::new();
        let mut hash = Sha256::new();
        let mut line = 1u64;
        let mut size = 0u64;

        let mut truncated = false;
        loop {
            let n = tokio::select! { _=cancel.cancelled()=>bail!("Cancelled"), n=file.read(&mut buf)=>n? };
            if n == 0 {
                break;
            }
            if buf[..n].contains(&0) {
                bail!("Read does not support binary files");
            }
            hash.update(&buf[..n]);
            size += n as u64;
            for &byte in &buf[..n] {
                if line >= start && line < end {
                    if selected.len() < READ_BYTES {
                        selected.push(byte);
                    } else {
                        truncated = true;
                    }
                }
                if byte == b'\n' {
                    line += 1;
                }
            }
        }
        let total = if size == 0 { 0 } else { line };
        let content = match String::from_utf8(selected) {
            Ok(text) => text,
            Err(e) if truncated && e.utf8_error().error_len().is_none() => {
                String::from_utf8(e.as_bytes()[..e.utf8_error().valid_up_to()].to_vec())?
            }
            Err(_) => bail!("Read requires valid UTF-8 text"),
        };
        let mut content = content.replace("\r\n", "\n");
        if start == 1 {
            content = content.trim_start_matches('\u{feff}').to_owned();
        }
        if content.ends_with('\n') && end <= total {
            content.pop();
        }
        let count = if content.is_empty() {
            0
        } else {
            content.split('\n').count()
        };
        let full = start == 1 && !truncated && count as u64 >= total;
        self.remember(path.clone(), hash.finalize().to_vec(), full)
            .await;
        let numbered = content
            .split('\n')
            .enumerate()
            .map(|(i, s)| format!("{}\t{s}", start + i as u64))
            .collect::<Vec<_>>()
            .join("\n");
        let model = if content.is_empty() {
            format!(
                "<system-reminder>{}</system-reminder>",
                if total == 0 {
                    "Warning: the file exists but the contents are empty.".to_owned()
                } else {
                    format!(
                        "Warning: the file exists but is shorter than the provided offset ({start}). The file has {total} lines."
                    )
                }
            )
        } else if truncated {
            format!(
                "<system-reminder>Partial view; use offset and limit to continue reading.</system-reminder>\n\n{numbered}"
            )
        } else {
            numbered
        };
        Ok(ToolOutput::new(
            model,
            json!({"type":"text","filePath":path,"content":content,"startLine":start,"numLines":count,"totalLines":total,"sizeBytes":size,"bytesRead":size,"truncated":truncated}),
        ))
    }
    async fn write(
        &self,
        name: &str,
        input: &Path,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let path = match zcode_cli_host::realpath(input).await {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => input.to_owned(),
            Err(e) => return Err(e.into()),
        };
        let original = match tokio::fs::metadata(&path).await {
            Ok(meta) => {
                if !meta.is_file() {
                    bail!("Write/Edit requires a regular file");
                }
                if meta.len() > EDIT_BYTES {
                    bail!("File exceeds native edit budget (8 MiB)");
                }
                Some(tokio::fs::read(&path).await?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        check_cancel(cancel)?;
        if let Some(bytes) = &original {
            let state = self.state.lock().await;
            let read = state.entries.get(&path).context(if name == "Write" {
                "write_file_not_read: Read the file before overwriting"
            } else {
                "edit_file_not_read: Read the file before editing"
            })?;
            if name == "Write" && !read.full {
                bail!("write_partial_read: Read the complete file before overwriting");
            }
            if read.hash != Sha256::digest(bytes).as_slice() {
                bail!("{name}: stale_file; file changed since Read, read it again");
            }
        }
        let raw = std::str::from_utf8(original.as_deref().unwrap_or_default())
            .context("Write/Edit requires UTF-8 text")?;
        if raw.contains('\0') {
            bail!("Write/Edit does not support binary files");
        }
        let bom = raw.starts_with('\u{feff}');
        let crlf = raw.contains("\r\n")
            || (original.is_none()
                && args[if name == "Write" {
                    "content"
                } else {
                    "new_string"
                }]
                .as_str()
                .is_some_and(|s| s.contains("\r\n")));
        let old = raw.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let mut matched: Option<(&str, usize)> = None;
        let replace_all = if name == "Edit" {
            boolean(args, "replace_all", false)?
        } else {
            false
        };
        let (new, search, replacement) = if name == "Write" {
            (
                string(args, "content")?.replace("\r\n", "\n"),
                String::new(),
                String::new(),
            )
        } else {
            let search = string(args, "old_string")?.replace("\r\n", "\n");
            let replacement = string(args, "new_string")?.replace("\r\n", "\n");
            if search == replacement {
                bail!("edit_no_change: old_string and new_string are identical");
            }
            if search.is_empty() {
                if !old.trim().is_empty() {
                    bail!("edit_file_exists_no_old_string");
                }
                (replacement.clone(), search, replacement)
            } else {
                // 修复：此前只做精确匹配，模型带弯引号/行号前缀/转义/缩进差异时 Node 能改而 Rust 失败；
                // 按 TS edit-matchers 的策略顺序匹配（docs/specs/rust-edit-matching.md）。
                let applied = edit_apply::apply(
                    &old,
                    &search,
                    &replacement,
                    string(args, "old_string")?,
                    replace_all,
                )?;
                matched = Some((applied.strategy.as_str(), applied.candidates));
                (applied.content, applied.actual_old, applied.actual_new)
            }
        };
        let new = match self.memory {
            // 按未 realpath 的请求路径判定（TS resolveWorkspacePath 不解析符号链接/短文件名）。
            Some((root, origin)) => crate::domain::memory::stamp_origin(
                &new,
                root,
                &input.to_string_lossy(),
                origin,
                &self.cwd.to_string_lossy(),
            ),
            None => new,
        };
        if new.len() as u64 > EDIT_BYTES {
            bail!("Write exceeds native edit budget (8 MiB)");
        }
        let mut bytes = if crlf {
            new.replace('\n', "\r\n").into_bytes()
        } else {
            new.as_bytes().to_vec()
        };
        if bom {
            bytes.splice(..0, [0xef, 0xbb, 0xbf]);
        }
        if let Some(sink) = self.sink {
            super::file_checkpoints::prepare(
                self.checkpoint_root,
                &path,
                name,
                original.as_deref(),
                &bytes,
                sink,
                cancel,
            )
            .await?;
        }
        super::file_atomic::atomic_write(&path, &bytes, original.as_deref(), cancel).await?;
        let path = zcode_cli_host::realpath(path).await?;
        self.remember(path.clone(), Sha256::digest(&bytes).to_vec(), true)
            .await;
        let (patch, additions, deletions) = patch(&old, &new);
        let mut data = if name == "Write" {
            json!({"type":if original.is_some(){"update"}else{"create"},"filePath":path,"content":new,"originalFile":original.as_ref().map(|_|&old),"structuredPatch":patch,"userModified":false})
        } else {
            let mut data = json!({"filePath":path,"oldString":search,"newString":replacement,"originalFile":old,"structuredPatch":patch,"userModified":false,"replaceAll":replace_all});
            // 与 TS 一致：新建/空文件（old_string 为空）不带匹配策略字段。
            if let Some((strategy, candidates)) = matched {
                data["matchStrategy"] = strategy.into();
                data["matchCandidateCount"] = candidates.into();
            }
            data
        };
        let mut display = json!({"kind":"file_diff","filePath":path,"additions":additions,"deletions":deletions,"structuredPatch":data["structuredPatch"]});
        if serde_json::to_vec(&display)?.len() > 32 * 1024 {
            display["structuredPatch"] = json!([]);
            display["truncated"] = true.into();
        }
        let mut content = format!(
            "The file {} has been {} successfully.",
            path.display(),
            if name == "Edit" { "updated" } else { "written" }
        );
        if serde_json::to_vec(&data)?.len() > 64 * 1024 {
            tokio::fs::create_dir_all(self.artifacts).await?;
            let artifact = self.artifacts.join(format!("{}.json", super::id()));
            tokio::fs::write(&artifact, serde_json::to_vec(&data)?).await?;
            content.push_str(&format!(" Full change result: {}", artifact.display()));
        }
        // data is adapter-local validation output; only bounded display/model text crosses stdio.
        let mut result = ToolOutput::new(content, std::mem::take(&mut data));
        result.display = Some(display);
        Ok(result)
    }
}
pub(super) fn patch(old: &str, new: &str) -> (Value, usize, usize) {
    if old == new {
        return (json!([]), 0, 0);
    }
    let a: Vec<_> = old.lines().collect();
    let b: Vec<_> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let removed = &a[prefix..a.len() - suffix];
    let added = &b[prefix..b.len() - suffix];
    let lines: Vec<_> = removed
        .iter()
        .map(|s| format!("-{s}"))
        .chain(added.iter().map(|s| format!("+{s}")))
        .collect();
    (
        json!([{"oldStart":prefix+1,"oldLines":removed.len(),"newStart":prefix+1,"newLines":added.len(),"lines":lines}]),
        added.len(),
        removed.len(),
    )
}
