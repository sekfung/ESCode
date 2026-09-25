//! 图片 Read 的模型预算压缩（docs/specs/rust-media-read.md），按 TS Jimp 适配器
//! `prepareJimpImageForModel` 的候选顺序与尺寸规则；编码器不同，压缩后的字节与 TS 不逐字相同。
use anyhow::{Result, bail};
use image::{
    DynamicImage, ImageFormat,
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType as PngFilter, PngEncoder},
    },
    imageops::FilterType,
};

pub(crate) const MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_BASE64: usize = 5 * 1024 * 1024;
const MAX_RAW: usize = MAX_BASE64 * 3 / 4;
const MAX_DIMENSION: u32 = 2000;
const MAX_TOKENS: usize = 25_000;
const JPEG_QUALITY: [u8; 4] = [80, 60, 40, 20];
const SCALES: [f64; 3] = [0.75, 0.5, 0.25];
const AGGRESSIVE: [u32; 6] = [1000, 800, 600, 400, 300, 200];
/// Jimp `getBuffer("image/jpeg")` 未指定质量时的默认值。
const JPEG_DEFAULT_QUALITY: u8 = 100;

pub(crate) struct Prepared {
    pub data: Vec<u8>,
    pub media_type: &'static str,
    pub strategy: &'static str,
    pub resized: bool,
    pub compressed: bool,
    pub original: Option<(u32, u32)>,
    pub size: Option<(u32, u32)>,
}

/// TS fitsImageBudget：原始字节、base64 字节与 token（base64 × 0.125 向上取整）三道预算。
fn fits(len: usize) -> bool {
    let base64 = len.div_ceil(3) * 4;
    len <= MAX_RAW && base64 <= MAX_BASE64 && base64.div_ceil(8) <= MAX_TOKENS
}
/// TS detectImageMediaType（文件头）。
pub(crate) fn detect(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
/// TS normalizeMediaType：非四种支持格式一律按 PNG 处理。
fn normalize(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "image/jpg" | "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => "image/png",
    }
}
fn js_round(value: f64) -> u32 {
    (value + 0.5).floor().max(0.0) as u32
}
/// Jimp scaleToFit(maxEdge, maxEdge) 的目标尺寸：`round(w·f)`、`round(h·f)`，0 取 1。
fn fit_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let (w, h, m) = (f64::from(width), f64::from(height), f64::from(max_edge));
    let factor = if 1.0 > w / h { m / h } else { m / w };
    (js_round(w * factor).max(1), js_round(h * factor).max(1))
}
fn longest(image: &DynamicImage) -> u32 {
    image.width().max(image.height())
}
fn resize_to_max_edge(image: &DynamicImage, max_edge: u32) -> DynamicImage {
    if longest(image) <= max_edge {
        return image.clone();
    }
    let (w, h) = fit_dimensions(image.width(), image.height(), max_edge);
    image.resize_exact(w, h, FilterType::CatmullRom)
}

struct Candidate {
    data: Vec<u8>,
    media_type: &'static str,
    strategy: &'static str,
    size: (u32, u32),
}
fn encode(image: &DynamicImage, media_type: &'static str, quality: u8) -> Result<Vec<u8>> {
    let mut out = vec![];
    match media_type {
        "image/jpeg" => {
            JpegEncoder::new_with_quality(&mut out, quality).encode_image(&image.to_rgb8())?
        }
        "image/png" => image.write_with_encoder(PngEncoder::new_with_quality(
            &mut out,
            CompressionType::Best,
            PngFilter::Adaptive,
        ))?,
        "image/gif" => image.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Gif)?,
        other => bail!("Unsupported image output {other}"),
    }
    Ok(out)
}
fn candidate(
    image: &DynamicImage,
    media_type: &'static str,
    quality: u8,
    strategy: &'static str,
) -> Result<Option<Candidate>> {
    let data = encode(image, media_type, quality)?;
    Ok(fits(data.len()).then(|| Candidate {
        data,
        media_type,
        strategy,
        size: (image.width(), image.height()),
    }))
}
fn jpeg_quality(image: &DynamicImage) -> Result<Option<Candidate>> {
    for quality in JPEG_QUALITY {
        if let Some(found) = candidate(image, "image/jpeg", quality, "jpeg-quality")? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}
fn format_preserving(image: &DynamicImage, source: &'static str) -> Result<Option<Candidate>> {
    match source {
        "image/png" => candidate(image, "image/png", 0, "png-optimized"),
        "image/jpeg" => jpeg_quality(image),
        "image/gif" => candidate(image, "image/gif", 0, "preserve-format"),
        _ => Ok(None),
    }
}
/// TS findFirstFittingCandidate（顺序逐条对应）。
fn first_fitting(image: &DynamicImage, source: &'static str) -> Result<Option<Candidate>> {
    let within = longest(image) <= MAX_DIMENSION;
    // PNG 只在原尺寸尝试一次无损优化，失败后单向转 JPEG。
    let preserve = source != "image/png";
    if within && let Some(found) = format_preserving(image, source)? {
        return Ok(Some(found));
    }
    let bounded = resize_to_max_edge(image, MAX_DIMENSION);
    let same = bounded.width() == image.width() && bounded.height() == image.height();
    if preserve && !same {
        let quality = JPEG_DEFAULT_QUALITY;
        if let Some(found) = candidate(&bounded, output_type(source), quality, "resized")? {
            return Ok(Some(found));
        }
    }
    if !within
        && preserve
        && let Some(found) = format_preserving(&bounded, source)?
    {
        return Ok(Some(found));
    }
    if let Some(found) = jpeg_quality(&bounded)? {
        return Ok(Some(found));
    }
    for scale in SCALES {
        let edge = js_round(f64::from(longest(&bounded)) * scale).max(1);
        let scaled = resize_to_max_edge(&bounded, edge);
        if preserve && let Some(found) = format_preserving(&scaled, source)? {
            return Ok(Some(found));
        }
        if let Some(found) = jpeg_quality(&scaled)? {
            return Ok(Some(found));
        }
    }
    for edge in AGGRESSIVE {
        let scaled = resize_to_max_edge(image, edge.min(MAX_DIMENSION));
        if let Some(found) = candidate(&scaled, "image/jpeg", 20, "jpeg-fallback")? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}
/// Jimp 输出类型：jpeg/png/gif 保持，其余按 PNG（TS jimpOutputMediaType）。
fn output_type(source: &'static str) -> &'static str {
    match source {
        "image/jpeg" | "image/gif" => source,
        _ => "image/png",
    }
}

/// TS prepareJimpImageForModel。错误文案与 TS 相同，调用方包装为工具失败。
pub(crate) fn prepare(input: Vec<u8>, requested: &str) -> Result<Prepared> {
    if input.is_empty() {
        bail!("Image file is empty (0 bytes)");
    }
    let source = detect(&input).unwrap_or_else(|| normalize(requested));
    if source == "image/webp" {
        if !fits(input.len()) {
            bail!(
                "WebP image exceeds the model image budget and the current image adapter cannot transcode WebP"
            );
        }
        return Ok(Prepared {
            data: input,
            media_type: "image/webp",
            strategy: "original",
            resized: false,
            compressed: false,
            original: None,
            size: None,
        });
    }
    let Ok(image) = image::load_from_memory(&input) else {
        bail!("Unable to decode image data");
    };
    let original = (image.width(), image.height());
    if longest(&image) <= MAX_DIMENSION && fits(input.len()) {
        return Ok(Prepared {
            data: input,
            media_type: source,
            strategy: "original",
            resized: false,
            compressed: false,
            original: Some(original),
            size: Some(original),
        });
    }
    let Some(found) = first_fitting(&image, source)? else {
        bail!(
            "Unable to compress image ({} bytes) within the requested model image budget",
            input.len()
        );
    };
    Ok(Prepared {
        resized: found.size != original,
        compressed: found.data.len() < input.len() || found.media_type != source,
        data: found.data,
        media_type: found.media_type,
        strategy: found.strategy,
        original: Some(original),
        size: Some(found.size),
    })
}

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
