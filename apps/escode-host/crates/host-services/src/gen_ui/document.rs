//! `gen-ui.readDocument` 的受限读取（TS `gen-ui/adapters/files.ts`）。
//!
//! 这是本域的安全边界，校验顺序即语义，逐条复刻 TS（`rust-u2-gen-ui.md` §3.1）：
//! 前缀 → **逐级拒符号链接** → 再看前缀 → 常规文件 → 打开后核对文件身份 → 定长读 → 严格 UTF-8 → 片段校验。
//! 「realpath 后再判前缀」会放行替换中的符号链接，所以必须**逐级** lstat 判链 + 打开后比对 dev/ino。

use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::paths::{child_of, is_separator, normalize_path};
use super::schema::{Target, parse_target};

/// TS `GEN_UI_HTML_MAX_BYTES`。
pub const HTML_MAX_BYTES: usize = 5_000_000;

#[derive(Debug)]
pub struct Document {
    pub path: String,
    pub html: String,
}

/// TS `node:path.isAbsolute`。
fn is_absolute(path: &str) -> bool {
    if path.starts_with('/') {
        return true;
    }
    if cfg!(windows) {
        if path.starts_with("\\\\") {
            return true;
        }
        let bytes = path.as_bytes();
        return bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
    }
    false
}

/// JS `\s` 的口径（与 Rust 的 `char::is_whitespace` **不同**）：含 `\uFEFF`、不含 `\u0085`。
fn is_js_space(character: char) -> bool {
    matches!(
        character,
        '\u{9}'..='\u{d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `\b` 在 JS 里由 `\w`（`[A-Za-z0-9_]`，**仅 ASCII**）定义，与 Unicode 词边界无关。
fn is_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn starts_with_ci(text: &str, needle: &str) -> bool {
    text.len() >= needle.len()
        && text.is_char_boundary(needle.len())
        && text[..needle.len()].eq_ignore_ascii_case(needle)
}

/// TS `assertGenUiFragment`：`/<(?:!doctype\b|\/?(?:html|head|body)(?:\s|>))/iu`。
fn has_document_tag(html: &str) -> bool {
    let mut search_from = 0;
    while let Some(offset) = html[search_from..].find('<') {
        let start = search_from + offset;
        let after = &html[start + 1..];
        if starts_with_ci(after, "!doctype") {
            match after["!doctype".len()..].chars().next() {
                None => return true,
                Some(next) if !is_word_char(next) => return true,
                Some(_) => {}
            }
        }
        let tags = after.strip_prefix('/').unwrap_or(after);
        for tag in ["html", "head", "body"] {
            if !starts_with_ci(tags, tag) {
                continue;
            }
            match tags[tag.len()..].chars().next() {
                None => return true,
                Some(next) if is_js_space(next) || next == '>' => return true,
                Some(_) => {}
            }
        }
        search_from = start + 1;
    }
    false
}

/// TS `assertGenUiFragment`。
pub fn assert_fragment(html: &str) -> Result<(), String> {
    if html.len() > HTML_MAX_BYTES {
        return Err("Gen UI file exceeds 5 MB".to_owned());
    }
    if has_document_tag(html) {
        return Err("Gen UI requires an HTML fragment".to_owned());
    }
    Ok(())
}

/// 打开前后的文件身份：Unix 比 `dev`/`ino`。
///
/// 修复：Windows 曾用 `volume_serial_number`/`file_index`，它们是不稳定特性（`windows_by_handle`），
/// 稳定工具链无法编译（checks 扩到 windows-latest 后首次暴露）；且两者只在句柄元数据上有值，
/// `symlink_metadata` 取到的 `before` 恒为 None，读取会永远报 “changed while opening”。
/// Windows 改由 [`open_without_following`] 的 `FILE_FLAG_OPEN_REPARSE_POINT` 防“检查后换成链接”：
/// 被换成链接时句柄指向链接本身，`is_file()` 拒绝。
fn identity(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

fn open_without_following(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT：不跟随重解析点（符号链接 / junction），与 O_NOFOLLOW 同义。
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

/// TS `readGenUiDocument`。`output_root` 由调用方给出（Host 固定值）。
pub fn read_document(raw: &Value, output_root: &str) -> Result<Document, String> {
    let target: Target = parse_target(raw)?;
    if !is_absolute(output_root)
        || !is_absolute(&target.path)
        || !is_absolute(&target.scope.workspace_path)
    {
        return Err("Invalid executor path".to_owned());
    }
    let root = normalize_path(output_root);
    let file = normalize_path(&target.path);
    // 读取范围只由输出根决定（**不**限 scope 子目录）：fork 会继承父会话的 HTML 路径。
    let child = child_of(&root, &file)
        .ok_or_else(|| "Gen UI file is outside the host output directory".to_owned())?;

    let canonical_root = std::fs::canonicalize(&root)
        .map_err(|error| format!("Gen UI output directory is unavailable: {error}"))?;
    // 逐级拒链：realpath 之后再判前缀会放行"检查后被换成链接"的路径。
    let mut current = canonical_root.clone();
    for segment in child.split(is_separator).filter(|part| !part.is_empty()) {
        current = current.join(segment);
        let meta = std::fs::symlink_metadata(&current).map_err(|error| format!("{error}"))?;
        if meta.file_type().is_symlink() {
            return Err("Gen UI does not read symbolic links".to_owned());
        }
    }

    let before = std::fs::symlink_metadata(&current).map_err(|error| format!("{error}"))?;
    let canonical_file = std::fs::canonicalize(&current).map_err(|error| format!("{error}"))?;
    let canonical_child = child_of(
        &canonical_root.to_string_lossy(),
        &canonical_file.to_string_lossy(),
    )
    .ok_or_else(|| "Gen UI file escaped the output directory".to_owned())?;
    let _ = canonical_child;
    if !before.is_file() || before.len() > HTML_MAX_BYTES as u64 {
        return Err("Gen UI requires a regular file under 5 MB".to_owned());
    }

    let mut handle = open_without_following(&current).map_err(|error| format!("{error}"))?;
    let opened = handle.metadata().map_err(|error| format!("{error}"))?;
    if !opened.is_file() || identity(&opened) != identity(&before) {
        return Err("Gen UI file changed while opening".to_owned());
    }

    // 定长读：多读 1 字节用来判定"超出上限"，避免把整个超大文件读进内存。
    let mut bytes = Vec::with_capacity(HTML_MAX_BYTES + 1);
    let read = handle
        .by_ref()
        .take(HTML_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{error}"))?;
    if read > HTML_MAX_BYTES {
        return Err("Gen UI file exceeds 5 MB".to_owned());
    }
    let html = String::from_utf8(bytes)
        .map_err(|error| format!("Gen UI file is not valid UTF-8: {error}"))?;
    assert_fragment(&html)?;
    Ok(Document { path: file, html })
}

/// 供测试与调用方复用的路径拼接（与 TS 同义）。
pub fn join_path(root: &str, child: &str) -> PathBuf {
    let mut path = PathBuf::from(root);
    for segment in child.split(is_separator).filter(|part| !part.is_empty()) {
        path.push(segment);
    }
    path
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
