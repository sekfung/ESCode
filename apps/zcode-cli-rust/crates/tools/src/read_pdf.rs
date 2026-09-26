//! Read 的 PDF 分支（docs/specs/rust-media-read.md 第 2 期），对齐 TS `read-pdf.ts` 与 Poppler 适配器：
//! - 无 `pages`：原生 PDF（≤20MB，pdfinfo 可用时 ≤10 页），模型收到说明文本 + file 块；
//! - 有 `pages`：pdftoppm 以 100 DPI 渲染 JPEG，逐页按图片预算压缩，模型收到说明文本 + 各页 image 块。
use anyhow::{Result, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

const NATIVE_MAX_BYTES: u64 = 20 * 1024 * 1024;
const EXTRACT_MAX_BYTES: u64 = 100 * 1024 * 1024;
const NATIVE_MAX_PAGES: u64 = 10;
const MAX_PAGES_PER_REQUEST: u64 = 20;
const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(5);
const RENDER_TIMEOUT: Duration = Duration::from_secs(120);

/// TS formatFileSize（toFixed(1) 去掉 `.0`）。
pub(crate) fn file_size(bytes: u64) -> String {
    let one = |value: f64| {
        let text = format!("{:.1}", (value * 10.0).round() / 10.0);
        text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
    };
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} bytes")
    } else if bytes < 1024 * 1024 {
        format!("{}KB", one(b / 1024.0))
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{}MB", one(b / (1024.0 * 1024.0)))
    } else {
        format!("{}GB", one(b / (1024.0 * 1024.0 * 1024.0)))
    }
}

/// TS parseReadPdfPageRange：`N`、`N-M`、`N-`（开放区间，最后一页为无穷）。
pub(crate) fn page_range(value: &str) -> Option<(u64, Option<u64>)> {
    let value = value.trim();
    if let Some(first) = value.strip_suffix('-')
        && !first.is_empty()
        && first.chars().all(|c| c.is_ascii_digit())
    {
        let first: u64 = first.parse().ok()?;
        return (first >= 1).then_some((first, None));
    }
    let (first, last) = match value.split_once('-') {
        Some((a, b)) => (a, b),
        None => (value, value),
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !digits(first) || !digits(last) {
        return None;
    }
    let (first, last): (u64, u64) = (first.parse().ok()?, last.parse().ok()?);
    (first >= 1 && last >= first).then_some((first, Some(last)))
}
/// TS getReadPdfPagesValidationFailure（仅 `.pdf` 且给了 pages 时）。
pub(crate) fn pages_validation(path: &str, pages: Option<&str>) -> Option<String> {
    let pages = pages?;
    if !path.to_lowercase().ends_with(".pdf") {
        return None;
    }
    match page_range(pages) {
        None => Some(format!(
            "Invalid pages parameter: \"{pages}\". Use formats like \"1-5\", \"3\", or \"10-20\". Pages are 1-indexed."
        )),
        Some((first, last))
            if last.map_or(MAX_PAGES_PER_REQUEST + 1, |l| l - first + 1)
                > MAX_PAGES_PER_REQUEST =>
        {
            Some(format!(
                "Page range \"{pages}\" exceeds maximum of {MAX_PAGES_PER_REQUEST} pages per request. Please use a smaller range."
            ))
        }
        Some(_) => None,
    }
}

/// TS ToolHandlerFailure：以 `<tool_use_error>` 包裹后回给模型。
fn failure(message: String) -> crate::contract::ToolOutput {
    let mut output =
        crate::contract::ToolOutput::text(format!("<tool_use_error>{message}</tool_use_error>"));
    output.failed = true;
    output
}
async fn run(file: &str, args: &[String], timeout: Duration) -> Option<std::process::Output> {
    let mut command = tokio::process::Command::new(file);
    zcode_cli_host::child_env::apply(&mut command, false);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()
}

pub(crate) async fn read(
    path: &Path,
    pages: Option<&str>,
    supports_image: bool,
    cancel: &CancellationToken,
) -> Result<crate::contract::ToolOutput> {
    if pages.is_some() && !supports_image {
        return Ok(failure(
            "The current model supports PDF input but does not support image input; remove the pages parameter."
                .into(),
        ));
    }
    let metadata = tokio::fs::metadata(path).await?;
    let display = path.to_string_lossy().into_owned();
    if !metadata.is_file() {
        return Ok(failure(format!("Path is not a regular file: {display}")));
    }
    if metadata.len() == 0 {
        return Ok(failure(format!("PDF file is empty: {display}")));
    }
    match pages {
        None => native(path, &display, metadata.len(), cancel).await,
        Some(pages) => rendered(&display, pages, metadata.len(), cancel).await,
    }
}

async fn native(
    path: &Path,
    display: &str,
    size: u64,
    cancel: &CancellationToken,
) -> Result<crate::contract::ToolOutput> {
    if size > NATIVE_MAX_BYTES {
        return Ok(failure(format!(
            "PDF file exceeds maximum allowed size of {}.",
            file_size(NATIVE_MAX_BYTES)
        )));
    }
    // pdfinfo 不可用时不限制页数（TS getPageCount 返回 undefined）。
    if let Some(output) = run("pdfinfo", &[display.to_owned()], INFO_TIMEOUT).await
        && output.status.success()
        && let Some(count) = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|l| {
                l.strip_prefix("Pages:")
                    .map(|v| v.trim().parse::<u64>().ok())
            })
            .flatten()
        && count > NATIVE_MAX_PAGES
    {
        return Ok(failure(format!(
            "This PDF has {count} pages, which is too many to read at once. Use the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Maximum {MAX_PAGES_PER_REQUEST} pages per request."
        )));
    }
    super::tools::check_cancel(cancel)?;
    let bytes = tokio::fs::read(path).await?;
    if !bytes.starts_with(b"%PDF-") {
        return Ok(failure(format!(
            "File is not a valid PDF (missing %PDF- header): {display}"
        )));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let intro = format!("PDF file read: {display} ({})", file_size(size));
    let url = format!(
        "data:application/pdf;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    let mut output = crate::contract::ToolOutput::new(
        format!("{intro}\n\n[Attached application/pdf: {name}]"),
        json!({"type":"pdf","filePath":display,"originalSize":size}),
    );
    output.media = vec![
        json!({"type":"text","text":intro}),
        json!({"type":"file","file":{"filename":name,"file_data":url},"_zcode_name":name}),
    ];
    Ok(output)
}

async fn rendered(
    display: &str,
    pages: &str,
    size: u64,
    cancel: &CancellationToken,
) -> Result<crate::contract::ToolOutput> {
    if size > EXTRACT_MAX_BYTES {
        return Ok(failure(format!(
            "PDF file exceeds maximum allowed size for text extraction ({}).",
            file_size(EXTRACT_MAX_BYTES)
        )));
    }
    if let Some(message) = pages_validation(display, Some(pages)) {
        return Ok(failure(message));
    }
    let (first, last) = page_range(pages).map(|(f, l)| (f, l.unwrap_or(f))).unwrap();
    let probe = run("pdftoppm", &["-v".into()], AVAILABILITY_TIMEOUT).await;
    let available = probe.as_ref().is_some_and(|o| {
        o.status.success() || (o.status.code().is_some_and(|c| c != 127) && !o.stderr.is_empty())
    });
    if !available {
        return Ok(failure(
            "pdftoppm is not installed. Install poppler-utils (e.g. `brew install poppler` or `apt-get install poppler-utils`) to enable PDF page rendering."
                .into(),
        ));
    }
    let directory = tempfile_dir()?;
    let prefix = directory.join("page");
    let args = [
        "-jpeg".into(),
        "-r".into(),
        "100".into(),
        "-f".into(),
        first.to_string(),
        "-l".into(),
        last.to_string(),
        display.to_owned(),
        prefix.to_string_lossy().into_owned(),
    ];
    let result = tokio::select! {biased;
        _ = cancel.cancelled() => { let _ = tokio::fs::remove_dir_all(&directory).await; bail!("Cancelled") }
        result = run("pdftoppm", &args, RENDER_TIMEOUT) => result,
    };
    let outcome = collect_pages(&directory, result, first, last).await;
    let _ = tokio::fs::remove_dir_all(&directory).await;
    let pages_data = match outcome {
        Ok(pages) => pages,
        Err(message) => return Ok(failure(message)),
    };
    let mut media = vec![];
    let count = pages_data.len();
    let intro = format!(
        "PDF pages extracted: {count} page(s) from {display} ({})",
        file_size(size)
    );
    media.push(json!({"type":"text","text":intro}));
    let mut placeholders = vec![intro.clone()];
    for (number, bytes) in pages_data {
        let prepared =
            tokio::task::spawn_blocking(move || zcode_cli_host::image_budget::prepare(bytes, "image/jpeg"))
                .await??;
        let mime = prepared.media_type;
        let name = format!("PDF page {number}");
        placeholders.push(format!("[Attached {mime}: {name}]"));
        let url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&prepared.data)
        );
        media.push(json!({"type":"image_url","image_url":{"url":url},"_zcode_name":name}));
    }
    let mut output = crate::contract::ToolOutput::new(
        placeholders.join("\n\n"),
        json!({"type":"parts","filePath":display,"numParts":count,"originalSize":size}),
    );
    output.media = media;
    Ok(output)
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let directory = std::env::temp_dir().join(format!("zcode-read-pdf-{}", super::id()));
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

/// 渲染结果分类与读取（TS assertRenderSucceeded + readRenderedPages）；Err 为给模型的失败文案。
async fn collect_pages(
    directory: &Path,
    result: Option<std::process::Output>,
    first: u64,
    last: u64,
) -> std::result::Result<Vec<(u64, Vec<u8>)>, String> {
    let Some(output) = result else {
        return Err(format!(
            "PDF page extraction timed out after {}ms.",
            RENDER_TIMEOUT.as_millis()
        ));
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(render_failure(&stderr, &output.stdout, first, last));
    }
    let mut pages = vec![];
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .map_err(|_| "Unable to list rendered PDF page images.".to_owned())?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name
            .to_lowercase()
            .strip_suffix(".jpg")
            .and_then(|stem| stem.rsplit_once('-'))
            .and_then(|(_, n)| n.parse::<u64>().ok())
            .filter(|n| *n >= 1)
        else {
            continue;
        };
        let bytes = tokio::fs::read(entry.path())
            .await
            .map_err(|_| "Unable to read rendered PDF page images.".to_owned())?;
        pages.push((number, bytes));
    }
    if pages.is_empty() {
        return Err("pdftoppm produced no output pages. The PDF may be invalid.".into());
    }
    pages.sort_by_key(|(n, _)| *n);
    Ok(pages)
}

fn render_failure(stderr: &str, stdout: &[u8], first: u64, last: u64) -> String {
    if stderr.to_lowercase().contains("password") {
        return "PDF is password-protected. Please provide an unprotected version.".into();
    }
    if let Some(index) = stderr.find("Wrong page range given")
        && let Some(count) = stderr[index..]
            .split("last page (")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .and_then(|n| n.trim().parse::<u64>().ok())
    {
        if count == 0 {
            return "PDF reports 0 pages (empty page tree). The PDF may be invalid.".into();
        }
        let requested = if first == last {
            format!("page {first}")
        } else {
            format!("pages {first}-{last}")
        };
        let plural = if count == 1 { "" } else { "s" };
        let example = count.min(MAX_PAGES_PER_REQUEST);
        return format!(
            "Requested {requested} is outside the document (PDF has {count} page{plural}). Use a range within 1-{count}, maximum {MAX_PAGES_PER_REQUEST} pages per request (e.g. pages: \"1-{example}\")."
        );
    }
    let lower = stderr.to_lowercase();
    if lower.contains("damaged") || lower.contains("corrupt") || lower.contains("invalid") {
        return "PDF file is corrupted or invalid.".into();
    }
    let detail = [stderr.trim(), String::from_utf8_lossy(stdout).trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if detail.is_empty() {
        "pdftoppm failed.".into()
    } else {
        format!("pdftoppm failed: {detail}")
    }
}

/// 模型输入能力（inputFormat）中的开关。
pub(crate) fn supports(input_format: &Value, key: &str) -> bool {
    input_format[key] == true
}
