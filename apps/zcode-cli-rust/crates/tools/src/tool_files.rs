use super::edit_apply;
pub use super::file_state::FileState;
use super::tools::{boolean, check_cancel, keys, resolve, string, uint};
use crate::contract::ToolOutput;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{

    path::{Path, PathBuf},
};
use tokio::{io::AsyncReadExt, sync::Mutex};
use tokio_util::sync::CancellationToken;
const READ_BYTES: usize = 64 * 1024;
const EDIT_BYTES: u64 = 8 * 1024 * 1024;
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
            // TS superRefine 先于文件访问：PDF 页码，再阻塞设备与二进制扩展名。
            let raw = string(args, "file_path")?;
            let pages = super::read_pdf::pages_validation(raw, args["pages"].as_str());
            if let Some(message) = pages.or_else(|| crate::domain::file_tool_text::read_path_refusal(raw)) {
                return Ok(super::read_pdf::failure(message));
            }
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
        self.state.lock().await.remember(path, hash, full);
    }
    async fn read(
        &self,
        path: &Path,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        // 修复：模型可见路径沿用 TS `resolveWorkspacePath` 的词法结果（不解析符号链接与 8.3 短名），
        // 此前整条链路 realpath，Windows（RUNNER~1→runneradmin）与 macOS（/var→/private/var）下
        // PDF 等输出里的路径与 Node 不一致，差分测试在两侧失败；realpath 只保留给读写状态键。
        // TS：不存在给出带相近文件名建议的文案；目录给出 adapter 文案（docs/specs/rust-file-tool-results.md）。
        match tokio::fs::metadata(path).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!(super::file_missing::message(self.cwd, path).await)
            }
            Ok(meta) if meta.is_dir() => bail!("Cannot read directory as text file: {}", path.display()),
            _ => {}
        }
        let state_key = zcode_cli_host::realpath(path).await?;
        // TS 先按扩展名分派媒体（docs/specs/rust-media-read.md）。
        if let Some(mime) = super::read_image::mime_from_path(path) {
            return super::read_image::read(path, mime, cancel).await;
        }
        if let Some(mime) = super::read_image::video_mime_from_path(path) {
            return super::read_image::read_video(path, mime, cancel).await;
        }
        if super::read_pdf::supports(&self.input_format, "supportsPdf")
            && path.to_string_lossy().to_lowercase().ends_with(".pdf")
        {
            let pages = args["pages"].as_str();
            let image = super::read_pdf::supports(&self.input_format, "supportsImage");
            return super::read_pdf::read(path, pages, image, cancel).await;
        }
        if !tokio::fs::metadata(&path).await?.is_file() {
            bail!("Read requires a regular file");
        }
        // TS：同一路径与范围（offset ?? 1, limit）的上次读取后文件未变，返回提示而不是重复正文。
        let range = (args["offset"].as_u64().unwrap_or(1), args["limit"].as_u64());
        let stamp = super::file_state::Stamp::of(&state_key).await;
        if let Some(stamp) = stamp
            && self.state.lock().await.range_fresh(&state_key, range.0, range.1, stamp)
        {
            let data = json!({"type": "file_unchanged", "filePath": path});
            return Ok(ToolOutput::new(crate::domain::file_tool_text::FILE_UNCHANGED.into(), data));
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
        // TS adapter 把空文件计为 1 行（提醒为 offset 超出而非空文件），Rust 之前计 0 行。
        let total = line;
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
        if let Some(stamp) = stamp.filter(|_| !truncated) {
            self.state.lock().await.record_range(state_key.clone(), range.0, range.1, stamp);
        }
        self.remember(state_key, hash.finalize().to_vec(), full)
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
        use crate::contract::ToolHandlerFailure as Failure;
        use crate::domain::file_tool_text as text;
        let edit = name == "Edit";
        // TS edit handler 先判断 old_string 与 new_string 相同，再解析路径与读文件。
        if edit && args["old_string"] == args["new_string"] {
            return Err(Failure(text::NO_CHANGE.into()).into());
        }
        let path = match zcode_cli_host::realpath(input).await {
            Ok(p) => p,
            // 新文件：规范化最近的已存在祖先，检查点与回退按同一工作区形态比较（文件工具的 cwd 为词法路径）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => zcode_cli_host::realpath_for_create(input).await,
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
        let empty_old = args["old_string"].as_str() == Some("");
        if edit && original.is_none() && !empty_old {
            return Err(Failure(super::file_missing::message(self.cwd, input).await).into());
        }
        // TS：old_string 为空而文件已有内容时，先于读状态检查报错。
        let blank = |b: &Vec<u8>| String::from_utf8_lossy(b).trim().is_empty();
        if edit && empty_old && original.as_ref().is_some_and(|b| !blank(b)) {
            return Err(Failure(text::EXISTS_NO_OLD_STRING.into()).into());
        }
        if let Some(bytes) = &original {
            // TS：Write 的未读/过期为普通错误，Edit 的为处理器失败（<tool_use_error>）。
            let fail = |message: &str| -> anyhow::Error {
                if edit { Failure(message.into()).into() } else { anyhow::anyhow!(message.to_owned()) }
            };
            let state = self.state.lock().await;
            match state.observation(&path) {
                Some(read) if read.full || edit => {
                    if read.hash != Sha256::digest(bytes).as_slice() {
                        return Err(fail(text::STALE));
                    }
                }
                _ => return Err(fail(text::NOT_READ)),
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
            if search.is_empty() {
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
        // 模型可见路径用词法请求路径（TS `resolveWorkspacePath` 不解析符号链接/短名）；
        // realpath 后的 path 只用于读状态键与检查点（checkpoint 恢复要求已归一）。
        // TS 的 filePath 与成功文案都用模型给出的原始 file_path（docs/specs/rust-file-tool-results.md）。
        let requested = std::path::PathBuf::from(string(args, "file_path")?);
        let path = zcode_cli_host::realpath(path).await?;
        self.remember(path.clone(), Sha256::digest(&bytes).to_vec(), true)
            .await;
        // TS updateReadFileStateAfterWrite：写后按整文件读取记录，紧接着的 Read 返回未变提示。
        if let Some(stamp) = super::file_state::Stamp::of(&path).await {
            self.state.lock().await.record_range(path.clone(), 1, None, stamp);
        }
        let (patch, additions, deletions) = super::file_patch::patch(&old, &new);
        let mut data = if name == "Write" {
            json!({"type":if old.is_empty(){"create"}else{"update"},"filePath":requested,"content":new,"originalFile":original.as_ref().map(|_|&old),"structuredPatch":patch,"userModified":false})
        } else {
            let mut data = json!({"filePath":requested,"oldString":search,"newString":replacement,"originalFile":old,"structuredPatch":patch,"userModified":false,"replaceAll":replace_all});
            // 与 TS 一致：新建/空文件（old_string 为空）不带匹配策略字段。
            if let Some((strategy, candidates)) = matched {
                data["matchStrategy"] = strategy.into();
                data["matchCandidateCount"] = candidates.into();
            }
            data
        };
        let mut display = json!({"kind":"file_diff","filePath":requested,"additions":additions,"deletions":deletions,"structuredPatch":data["structuredPatch"]});
        if serde_json::to_vec(&display)?.len() > 32 * 1024 {
            display["structuredPatch"] = json!([]);
            display["truncated"] = true.into();
        }
        let shown = requested.to_string_lossy();
        let mut content = if edit {
            text::edit_success(&shown, replace_all)
        } else {
            // TS `if (originalFile)`：已存在但为空的文件按新建处理。
            text::write_success(&shown, old.is_empty())
        };
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
