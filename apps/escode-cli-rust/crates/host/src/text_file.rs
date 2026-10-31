//! TS `readTextFileForModel` + adapters `readTextFileRange` 对齐（docs/specs/rust-file-tool-results.md）。
//! 旧实现边读边取 64 KiB 并给出自拟的 partial view；Node 对未给 limit 的整文件读取在 256 KiB 处报错，
//! 超 token 预算时给部分视图或报错。编码探测、CRLF 归一、行号与去重键也在本模块内对齐。
use anyhow::{Result, bail};
use encoding_rs::{GB18030, GBK, UTF_8, UTF_16LE};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::text_file_tokens::{estimate_tokens, partial_view};

/// TS 适配器快路径阈值：不超过 10 MiB 时整文件读取并按全文探测编码。
const FAST_PATH_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// TS `READ_MAX_FILE_SIZE_BYTES`。
pub const READ_MAX_FILE_SIZE_BYTES: u64 = 256 * 1024;
/// TS `READ_DEFAULT_MAX_LINES`。
pub const READ_DEFAULT_MAX_LINES: u64 = 2_000;
/// TS `READ_MAX_OUTPUT_TOKENS`。
pub(super) const READ_MAX_OUTPUT_TOKENS: usize = 25_000;
/// TS `Math.floor(READ_MAX_OUTPUT_TOKENS * 0.85)`。
pub(super) const PARTIAL_TARGET: usize = READ_MAX_OUTPUT_TOKENS * 85 / 100;
/// TS `ENCODING_SAMPLE_BYTES`。
const ENCODING_SAMPLE_BYTES: usize = 4_096;

pub struct ReadText {
    pub content: String,
    pub start_line: u64,
    pub num_lines: u64,
    pub total_lines: u64,
    pub size_bytes: u64,
    pub bytes_read: u64,
    pub truncated: bool,
    pub truncated_by_token_cap: bool,
    pub partial_view_notice: Option<String>,
    pub digest: Vec<u8>,
    pub covers_whole_file: bool,
}

/// TS `readTextFileForModel`：`offset`/`limit` 语义与 `startLine`（offset=0 时为 0）一致。
pub async fn read(
    path: &Path,
    offset: Option<u64>,
    limit: Option<u64>,
    allow_partial_fallback: bool,
    cancel: &CancellationToken,
) -> Result<ReadText> {
    let size_bytes = tokio::fs::metadata(path).await?.len();
    // TS `maxBytes`：只有未提供 limit 的整文件读取才受 256 KiB 限制。
    if limit.is_none() && size_bytes > READ_MAX_FILE_SIZE_BYTES {
        bail!(
            "File content ({}) exceeds maximum allowed size ({}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.",
            format_size(size_bytes),
            format_size(READ_MAX_FILE_SIZE_BYTES)
        );
    }
    let offset_line = offset.unwrap_or(1).saturating_sub(1);
    let (content, total_lines, digest) = if size_bytes <= FAST_PATH_MAX_BYTES {
        let bytes = tokio::fs::read(path).await?;
        let encoding = detect(&bytes, path)?;
        let text = encoding.decode(&bytes);
        let normalized = text.replace("\r\n", "\n");
        let lines: Vec<&str> = if normalized.is_empty() {
            Vec::new()
        } else {
            normalized.split('\n').collect()
        };
        let selected = select(&lines, offset_line, limit);
        (
            selected.join("\n"),
            lines.len() as u64,
            Sha256::digest(&bytes).to_vec(),
        )
    } else {
        stream(path, offset_line, limit, cancel).await?
    };

    let token_count = estimate_tokens(&content);
    let mut content = content;
    let mut num_lines = if content.is_empty() {
        0
    } else {
        content.split('\n').count() as u64
    };
    let mut total_lines = total_lines;
    let start_line = if offset == Some(0) {
        0
    } else {
        offset_line + 1
    };
    let mut truncated = false;
    let mut truncated_by_token_cap = false;
    let mut partial_view_notice = None;
    if token_count > READ_MAX_OUTPUT_TOKENS {
        if !allow_partial_fallback {
            bail!(
                "File content ({token_count} tokens) exceeds maximum allowed tokens ({READ_MAX_OUTPUT_TOKENS}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file."
            );
        }
        let partial = partial_view(&content, start_line, total_lines, token_count);
        content = partial.content;
        num_lines = partial.num_lines;
        truncated = true;
        truncated_by_token_cap = true;
        partial_view_notice = Some(partial.notice);
    }
    // TS `normalizeReadTextOutput`：空文件计为 1 行（不适用于 offset=0/越界 offset）。
    if content.is_empty() && num_lines == 0 && start_line == 1 && total_lines == 0 {
        num_lines = 1;
        total_lines = 1;
    }
    let covers_whole_file = offset_line == 0 && !truncated_by_token_cap && num_lines >= total_lines;
    Ok(ReadText {
        content,
        start_line,
        num_lines,
        total_lines,
        size_bytes,
        bytes_read: size_bytes,
        truncated,
        truncated_by_token_cap,
        partial_view_notice,
        digest,
        covers_whole_file,
    })
}

/// TS `formatReadTextOutput` 的入口（实现见 `read_text_format`）。
/// TS `isTextLikePath`：只有这些扩展名按文本读入 prompt，其余交付路径引用。
pub fn is_text_like_path(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "cjs", "conf", "cpp", "cs", "css", "csv", "go", "h", "hpp", "html", "ini", "java", "js",
        "json", "jsx", "log", "md", "mjs", "py", "rs", "sh", "sql", "toml", "ts", "tsx", "txt",
        "xml", "yaml", "yml",
    ];
    extension(path).is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?;
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

fn select<'a>(lines: &[&'a str], offset_line: u64, limit: Option<u64>) -> Vec<&'a str> {
    let start = usize::try_from(offset_line)
        .unwrap_or(usize::MAX)
        .min(lines.len());
    let end = limit
        .map(|limit| start.saturating_add(usize::try_from(limit).unwrap_or(usize::MAX)))
        .unwrap_or(lines.len())
        .min(lines.len());
    lines[start..end].to_vec()
}

async fn stream(
    path: &Path,
    offset_line: u64,
    limit: Option<u64>,
    cancel: &CancellationToken,
) -> Result<(String, u64, Vec<u8>)> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut head = vec![0u8; ENCODING_SAMPLE_BYTES];
    let read = file.read(&mut head).await?;
    head.truncate(read);
    let encoding = detect(&head, path)?;
    let mut hasher = Sha256::new();
    hasher.update(&head);
    if encoding.is_utf16le() {
        // UTF-16LE 的行边界是 2 字节对齐；直接把剩余内容读完再按全文语义切行。
        let mut bytes = head;
        file.read_to_end(&mut bytes).await?;
        hasher = Sha256::new();
        hasher.update(&bytes);
        let text = encoding.decode(&bytes);
        let normalized = text.replace("\r\n", "\n");
        let lines: Vec<&str> = if normalized.is_empty() {
            Vec::new()
        } else {
            normalized.split('\n').collect()
        };
        let selected = select(&lines, offset_line, limit);
        return Ok((
            selected.join("\n"),
            lines.len() as u64,
            hasher.finalize().to_vec(),
        ));
    }
    let mut selected = Vec::new();
    let mut pending = Vec::new();
    let mut line_index = 0u64;
    let mut saw_any = !head.is_empty();
    let mut buf = vec![0u8; 64 * 1024];
    let process =
        |bytes: &[u8], pending: &mut Vec<u8>, selected: &mut Vec<String>, line_index: &mut u64| {
            for &byte in bytes {
                if byte == b'\n' {
                    let raw = std::mem::take(pending);
                    push_line(encoding, &raw, *line_index, offset_line, limit, selected);
                    *line_index += 1;
                } else {
                    pending.push(byte);
                }
            }
        };
    process(&head, &mut pending, &mut selected, &mut line_index);
    loop {
        if cancel.is_cancelled() {
            bail!("Cancelled");
        }
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        saw_any = true;
        hasher.update(&buf[..read]);
        process(&buf[..read], &mut pending, &mut selected, &mut line_index);
    }
    if saw_any {
        push_line(
            encoding,
            &pending,
            line_index,
            offset_line,
            limit,
            &mut selected,
        );
        line_index += 1;
    }
    Ok((selected.join("\n"), line_index, hasher.finalize().to_vec()))
}

fn push_line(
    encoding: TextEncoding,
    raw: &[u8],
    line_index: u64,
    offset_line: u64,
    limit: Option<u64>,
    selected: &mut Vec<String>,
) {
    let end = limit.map_or(u64::MAX, |limit| offset_line.saturating_add(limit));
    if line_index < offset_line || line_index >= end {
        return;
    }
    selected.push(encoding.decode_line(raw));
}

#[derive(Clone, Copy)]
enum TextEncoding {
    Utf8,
    Utf16Le,
    Gbk,
    Gb18030,
}

impl TextEncoding {
    fn is_utf16le(self) -> bool {
        matches!(self, Self::Utf16Le)
    }
    fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
            Self::Utf16Le => {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            Self::Gbk => GBK.decode_without_bom_handling(bytes).0.into_owned(),
            Self::Gb18030 => GB18030.decode_without_bom_handling(bytes).0.into_owned(),
        }
    }
    fn decode_line(self, bytes: &[u8]) -> String {
        let text = self.decode(bytes);
        match text.strip_suffix('\r') {
            Some(text) => text.to_owned(),
            None => text,
        }
    }
    fn strict(self, bytes: &[u8]) -> Option<Cow<'_, str>> {
        match self {
            Self::Gbk => GBK.decode_without_bom_handling_and_without_replacement(bytes),
            Self::Gb18030 => GB18030.decode_without_bom_handling_and_without_replacement(bytes),
            _ => None,
        }
    }
    fn encode(self, text: &str) -> Cow<'_, [u8]> {
        match self {
            Self::Gbk => GBK.encode(text).0,
            Self::Gb18030 => GB18030.encode(text).0,
            Self::Utf8 => UTF_8.encode(text).0,
            Self::Utf16Le => UTF_16LE.encode(text).0,
        }
    }
}

fn detect(bytes: &[u8], path: &Path) -> Result<TextEncoding> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Ok(TextEncoding::Utf8);
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        return Ok(TextEncoding::Utf16Le);
    }
    if looks_like_binary(bytes) {
        return Err(unsupported(path));
    }
    if std::str::from_utf8(bytes).is_ok() {
        return Ok(TextEncoding::Utf8);
    }
    for encoding in [TextEncoding::Gbk, TextEncoding::Gb18030] {
        if round_trips(bytes, encoding) {
            return Ok(encoding);
        }
    }
    Err(unsupported(path))
}

/// TS `roundTripsWithOptionalTrailingDrop`：允许末尾最多 3 字节的截断。
fn round_trips(bytes: &[u8], encoding: TextEncoding) -> bool {
    if bytes.is_empty() {
        return true;
    }
    for drop in 0..=3.min(bytes.len()) {
        let candidate = &bytes[..bytes.len() - drop];
        if candidate.is_empty() {
            continue;
        }
        let Some(text) = encoding.strict(candidate) else {
            continue;
        };
        if encoding.encode(&text).as_ref() == candidate {
            return true;
        }
    }
    false
}

/// TS `looksLikeBinary`：NUL，或控制字符占比超过 30%。
fn looks_like_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut control = 0usize;
    for &byte in bytes {
        if byte == 0 {
            return true;
        }
        if byte < 0x09 || (byte > 0x0d && byte < 0x20) {
            control += 1;
        }
    }
    control * 10 > bytes.len() * 3
}

fn unsupported(path: &Path) -> anyhow::Error {
    anyhow::anyhow!("Unsupported or binary text encoding: {}", path.display())
}

/// TS `formatFileSize`（错误信息里的 256KB/…）。
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    if bytes < 1024 * 1024 {
        return format_unit(bytes as f64 / 1024.0, "KB");
    }
    format_unit(bytes as f64 / (1024.0 * 1024.0), "MB")
}

fn format_unit(value: f64, unit: &str) -> String {
    if value.fract() == 0.0 {
        return format!("{}{unit}", value as u64);
    }
    let text = format!("{value:.1}");
    format!("{}{unit}", text.trim_end_matches(".0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_ts_format() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024), "1KB");
        assert_eq!(format_size(540 * 1024 + 512), "540.5KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2MB");
    }

    #[test]
    fn selection_and_spans() {
        let lines = ["a", "b", "c", "d"];
        assert_eq!(select(&lines, 0, Some(2)), vec!["a", "b"]);
        assert_eq!(select(&lines, 1, None), vec!["b", "c", "d"]);
        assert!(select(&lines, 10, None).is_empty());
    }
}
