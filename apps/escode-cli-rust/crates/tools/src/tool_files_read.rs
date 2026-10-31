//! FileTools 的读取面（从 tool_files.rs 拆出）：媒体分派、读状态去重与分页截断。

use super::tool_files::FileTools;
use super::tools::uint;
use crate::contract::ToolOutput;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;

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
        // TS 去重键：(offset ?? 1, limit)；同一范围在上次读取后文件未变时返回提示而不是重复正文。
        let offset = match args.get("offset") {
            Some(_) => Some(uint(args, "offset", 0)?),
            None => None,
        };
        let limit = match args.get("limit") {
            Some(_) => {
                let value = uint(args, "limit", 0)?;
                if value == 0 {
                    bail!("limit must be positive");
                }
                Some(value)
            }
            None => None,
        };
        let range = (offset.unwrap_or(1), limit);
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
        // TS `allowPartialFallback` 缺省为初始整文件读取（offset 缺省/≤1 且未给 limit）。
        let allow_partial = limit.is_none() && offset.unwrap_or(1) <= 1;
        let read =
            escode_cli_host::text_file::read(path, offset, limit, allow_partial, cancel).await?;
        if let Some(stamp) = stamp {
            let mut state = self.state.lock().await;
            if read.truncated_by_token_cap {
                // TS：partial view 覆盖同键旧记录且永不命中缓存，第三次相同读取不得退回去重。
                state.invalidate_range(state_key.clone(), range.0, range.1);
            } else {
                state.record_range(state_key.clone(), range.0, range.1, stamp);
            }
        }
        self.remember(state_key, read.digest.clone(), read.covers_whole_file)
            .await;
        let model = super::read_text_format::format_model(&read);
        let mut data = json!({
            "type": "text",
            "filePath": path,
            "content": read.content,
            "startLine": read.start_line,
            "numLines": read.num_lines,
            "totalLines": read.total_lines,
            "sizeBytes": read.size_bytes,
            "bytesRead": read.bytes_read,
            "truncated": read.truncated,
        });
        if read.truncated_by_token_cap {
            data["truncatedByTokenCap"] = true.into();
        }
        if let Some(notice) = &read.partial_view_notice {
            data["partialViewNotice"] = notice.clone().into();
        }
        Ok(ToolOutput::new(model, data))
    }
}
