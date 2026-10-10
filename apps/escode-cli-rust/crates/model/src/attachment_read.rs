//! 文本附件的读取结果，对齐 TS `readTextFileForModel`（整文件读取、允许部分视图）与
//! adapters `readTextFileRange` 的快路径：CRLF 归一、按 `\n` 切行、超 256 KiB 只取前 2000 行、
//! 估算 token 超过 25000 时给出部分视图与提示。见 docs/specs/rust-attachment-prompt.md。

/// TS `READ_MAX_FILE_SIZE_BYTES`。
const READ_MAX_FILE_SIZE_BYTES: usize = 256 * 1024;
/// TS `READ_DEFAULT_MAX_LINES`。
const READ_DEFAULT_MAX_LINES: usize = 2_000;
/// TS `READ_MAX_OUTPUT_TOKENS`。
const READ_MAX_OUTPUT_TOKENS: usize = 25_000;
/// TS `Math.floor(READ_MAX_OUTPUT_TOKENS * 0.85)`。
const PARTIAL_TARGET: usize = READ_MAX_OUTPUT_TOKENS * 85 / 100;

pub(super) struct Read {
    pub content: String,
    pub partial_view_notice: Option<String>,
}

/// TS `estimateTokens`：按 UTF-16 长度计，`[一-鿿]` 计两个字符，除以 3 向上取整。
fn estimate_tokens(text: &str) -> usize {
    let mut weighted = 0usize;
    for c in text.chars() {
        weighted += if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            2
        } else {
            c.len_utf16()
        };
    }
    weighted.div_ceil(3)
}

pub(super) fn read(text: &str, size_bytes: usize) -> Read {
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = if normalized.is_empty() {
        vec![]
    } else {
        normalized.split('\n').collect()
    };
    let total = lines.len();
    let selected = if size_bytes > READ_MAX_FILE_SIZE_BYTES {
        &lines[..total.min(READ_DEFAULT_MAX_LINES)]
    } else {
        &lines[..]
    };
    let content = selected.join("\n");
    let tokens = estimate_tokens(&content);
    if tokens <= READ_MAX_OUTPUT_TOKENS {
        return Read {
            content,
            partial_view_notice: None,
        };
    }
    let head = format!(
        "The file is too large to display in full ({tokens} estimated tokens, limit {READ_MAX_OUTPUT_TOKENS})."
    );
    let count = largest_prefix(selected.len(), |n| {
        estimate_tokens(&selected[..n].join("\n")) <= PARTIAL_TARGET
    });
    if count > 0 {
        return Read {
            content: selected[..count].join("\n"),
            partial_view_notice: Some(format!(
                "{head} Showing a partial view of lines 1-{count} of {total}. Use Read with offset {} and limit {READ_DEFAULT_MAX_LINES} to continue, or use a search tool to find a specific section.",
                count + 1
            )),
        };
    }
    // 首行本身超出预算：TS 按 UTF-16 下标截取；这里按字符边界取不超过同一预算的最长前缀。
    let chars: Vec<char> = content.chars().collect();
    let count = largest_prefix(chars.len(), |n| {
        estimate_tokens(&chars[..n].iter().collect::<String>()) <= PARTIAL_TARGET
    });
    Read {
        content: chars[..count].iter().collect(),
        partial_view_notice: Some(format!(
            "{head} Showing a partial view of the first line because the first line alone exceeds the token budget. Use Read with a smaller range or use a search tool to find a specific section."
        )),
    }
}

/// TS `findLargestPrefixWithinTokenBudget` 的二分：满足 `fits` 的最大前缀长度。
fn largest_prefix(len: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// TS `isTextLikePath`：只有这些扩展名按文本读入 prompt，其余交付路径引用。
pub(super) fn is_text_like_path(path: &str) -> bool {
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

/// TS `resolvedPathReferenceAttachment` 的正文（reason = binary_file），mime 按 `inferAttachmentMimeFromPath`。
pub(super) fn path_reference(placeholder: &str) -> String {
    let lower = placeholder.to_ascii_lowercase();
    let video = [
        (".mp4", "video/mp4"),
        (".m4v", "video/x-m4v"),
        (".mov", "video/quicktime"),
        (".webm", "video/webm"),
        (".mkv", "video/x-matroska"),
        (".avi", "video/x-msvideo"),
    ]
    .into_iter()
    .find(|(ext, _)| lower.ends_with(ext))
    .map(|(_, mime)| mime);
    let mime = match extension(placeholder).as_deref() {
        Some("pdf") => "application/pdf",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("md") => "text/markdown",
        _ => video.unwrap_or(if is_text_like_path(placeholder) {
            "text/plain"
        } else {
            "application/octet-stream"
        }),
    };
    format!(
        "Attached {mime}: {placeholder}\nThe file was sent by local path because the file is not a known text attachment.\nUse the available file reading tools if you need to inspect the file contents."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_count_utf16_and_cjk_like_ts() {
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 2);
        assert_eq!(estimate_tokens("中"), 1);
        assert_eq!(estimate_tokens("中文"), 2);
        assert_eq!(estimate_tokens("😀"), 1);
    }

    #[test]
    fn small_files_keep_every_line_including_the_trailing_empty_one() {
        let r = read("a\r\nb\n", 5);
        assert_eq!(r.content, "a\nb\n");
        assert!(r.partial_view_notice.is_none());
    }

    #[test]
    fn oversized_token_count_yields_a_partial_view() {
        let text = (0..3000)
            .map(|i| format!("line {i} {}", "x".repeat(100)))
            .collect::<Vec<_>>()
            .join("\n");
        let r = read(&text, text.len());
        let notice = r.partial_view_notice.unwrap();
        assert!(notice.contains("of 3000."), "{notice}");
        assert!(estimate_tokens(&r.content) <= PARTIAL_TARGET);
    }

    #[test]
    fn text_like_extensions_follow_ts() {
        assert!(is_text_like_path("C:\\w\\a.TXT"));
        assert!(!is_text_like_path("/w/data.dat"));
        assert!(!is_text_like_path("/w.d/noext"));
        assert!(
            path_reference("/w/x.bin").starts_with("Attached application/octet-stream: /w/x.bin\n")
        );
    }
}
