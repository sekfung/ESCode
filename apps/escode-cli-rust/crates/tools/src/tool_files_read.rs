//! FileTools 的读取面（从 tool_files.rs 拆出）：媒体分派、读状态去重与分页截断。

use super::tool_files::FileTools;
use super::tools::uint;
use crate::contract::ToolOutput;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

const READ_BYTES: usize = 64 * 1024;

impl FileTools<'_> {
    pub(super) async fn read(
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
            Ok(meta) if meta.is_dir() => {
                bail!("Cannot read directory as text file: {}", path.display())
            }
            _ => {}
        }
        let state_key = escode_cli_host::realpath(path).await?;
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
            && self
                .state
                .lock()
                .await
                .range_fresh(&state_key, range.0, range.1, stamp)
        {
            let data = json!({"type": "file_unchanged", "filePath": path});
            return Ok(ToolOutput::new(
                crate::domain::file_tool_text::FILE_UNCHANGED.into(),
                data,
            ));
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
            self.state
                .lock()
                .await
                .record_range(state_key.clone(), range.0, range.1, stamp);
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
}
