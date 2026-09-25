//! 图片/视频 Read（docs/specs/rust-media-read.md）；预算压缩在 `zcode_cli_host::image_budget`。
use anyhow::{Result, bail};
use zcode_cli_host::image_budget::prepare;

pub(crate) const MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;

/// TS inferImageMimeFromPath（扩展名，不区分大小写）。
pub(crate) fn mime_from_path(path: &std::path::Path) -> Option<&'static str> {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        Some("image/jpeg")
    } else if lower.ends_with(".png") {
        Some("image/png")
    } else if lower.ends_with(".gif") {
        Some("image/gif")
    } else if lower.ends_with(".webp") {
        Some("image/webp")
    } else {
        None
    }
}

/// Read 的图片分支（TS readImageFile + formatReadImageOutput）：模型只收到一个 image 块，
/// 尺寸等结构化信息留在 data。压缩在阻塞线程执行。
pub(crate) async fn read(
    path: &std::path::Path,
    mime: &'static str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::contract::ToolOutput> {
    use base64::Engine as _;
    let size = tokio::fs::metadata(path).await?.len();
    if size > MAX_INPUT_BYTES {
        bail!(
            "File is too large to read as an image ({size} bytes, maximum {MAX_INPUT_BYTES} bytes)"
        );
    }
    let bytes = tokio::fs::read(path).await?;
    super::tools::check_cancel(cancel)?;
    let prepared = tokio::task::spawn_blocking(move || prepare(bytes, mime)).await??;
    super::tools::check_cancel(cancel)?;
    let media_type = prepared.media_type;
    let url = format!(
        "data:{media_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&prepared.data)
    );
    let dimensions = match (prepared.original, prepared.size) {
        (Some((ow, oh)), Some((w, h))) => serde_json::json!({
            "originalWidth": ow, "originalHeight": oh, "displayWidth": w, "displayHeight": h,
        }),
        _ => serde_json::json!({}),
    };
    let data = serde_json::json!({
        "type": "image",
        "mimeType": media_type,
        "originalSize": size,
        "transformedSize": prepared.data.len(),
        "resized": prepared.resized,
        "compressed": prepared.compressed,
        "compressionStrategy": prepared.strategy,
        "dimensions": dimensions,
    });
    let mut output =
        crate::contract::ToolOutput::new(format!("[Attached {media_type}: Read image]"), data);
    output.media = vec![serde_json::json!({
        "type": "image_url",
        "image_url": {"url": url},
        "_zcode_name": "Read image",
    })];
    Ok(output)
}

const VIDEO_MAX_BYTES: u64 = 30 * 1024 * 1024;
/// TS inferVideoMimeFromPath（VIDEO_INPUT_MIME_BY_EXTENSION）。
pub(crate) fn video_mime_from_path(path: &std::path::Path) -> Option<&'static str> {
    let lower = path.to_string_lossy().to_lowercase();
    [
        (".mp4", "video/mp4"),
        (".m4v", "video/x-m4v"),
        (".mov", "video/quicktime"),
        (".webm", "video/webm"),
        (".mkv", "video/x-matroska"),
        (".avi", "video/x-msvideo"),
    ]
    .into_iter()
    .find(|(extension, _)| lower.ends_with(extension))
    .map(|(_, mime)| mime)
}
/// Read 的视频分支（TS readVideoFile + formatReadVideoOutput）：不转码，base64 直传并校验大小。
pub(crate) async fn read_video(
    path: &std::path::Path,
    mime: &'static str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<crate::contract::ToolOutput> {
    use base64::Engine as _;
    let size = tokio::fs::metadata(path).await?.len();
    if size > VIDEO_MAX_BYTES {
        bail!(
            "File is too large to read as a video ({size} bytes, maximum {VIDEO_MAX_BYTES} bytes)"
        );
    }
    let bytes = tokio::fs::read(path).await?;
    super::tools::check_cancel(cancel)?;
    if bytes.is_empty() {
        bail!("Cannot read an empty video file.");
    }
    let url = format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    let mut output = crate::contract::ToolOutput::new(
        format!("[Attached {mime}: Read video]"),
        serde_json::json!({"type":"video","mimeType":mime,"originalSize":size}),
    );
    output.media = vec![serde_json::json!({
        "type": "video_url",
        "video_url": {"url": url},
        "_zcode_name": "Read video",
    })];
    Ok(output)
}
