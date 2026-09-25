//! Composer 图片附件的模型预算（docs/specs/rust-media-read.md 第 4 期，对齐 TS
//! `attachment-image.ts#prepareImageDataUrl`）：与 Read 共用 `image_budget::prepare`
//! （2000 边长、3.75MB 原始字节、5MB base64）。
//!
//! 附件快照不可变，结果按快照缓存：长历史每个请求都会重新物化全部附件，不能每步重新编码。
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};
use zcode_cli_host::image_budget;

/// 进程内缓存上限（按准备后的字节计）；淘汰只会导致重新编码，不影响结果。
const CACHE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct PreparedImage {
    pub media_type: String,
    pub data: Vec<u8>,
}
type Entry = (String, Arc<Option<PreparedImage>>);

fn cache() -> &'static Mutex<VecDeque<Entry>> {
    static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 返回预算内的图片；无法解码或压不进预算时为 `None`（调用方按 TS 以占位文本交付）。
pub(super) async fn prepare(key: String, bytes: Vec<u8>, mime: &str) -> Arc<Option<PreparedImage>> {
    if let Some((_, hit)) = cache().lock().unwrap().iter().find(|(k, _)| *k == key) {
        return hit.clone();
    }
    let requested = mime.to_owned();
    let prepared = tokio::task::spawn_blocking(move || {
        image_budget::prepare(bytes, &requested)
            .ok()
            .map(|p| PreparedImage {
                media_type: p.media_type.to_owned(),
                data: p.data,
            })
    })
    .await
    .ok()
    .flatten();
    let prepared = Arc::new(prepared);
    let mut entries = cache().lock().unwrap();
    entries.push_back((key, prepared.clone()));
    let size = |e: &Entry| e.1.as_ref().as_ref().map_or(0, |p| p.data.len());
    while entries.iter().map(size).sum::<usize>() > CACHE_BYTES && entries.len() > 1 {
        entries.pop_front();
    }
    prepared
}

/// TS `derivedMediaAttachmentPath` 的缓存根（`<storageRoot>/cli`）；启动时由 main 设置，未设置时不派生路径。
static MEDIA_CACHE_ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
pub fn set_media_cache_root(root: std::path::PathBuf) {
    let _ = MEDIA_CACHE_ROOT.set(root);
}

/// TS `extensionForDerivedMediaContentType`。
fn derived_extension(mime: &str) -> Option<(&'static str, &'static str)> {
    let normalized = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    Some(match normalized.as_str() {
        "image/png" => ("image", ".png"),
        "image/jpeg" | "image/jpg" => ("image", ".jpg"),
        "image/gif" => ("image", ".gif"),
        "image/webp" => ("image", ".webp"),
        "video/mp4" => ("video", ".mp4"),
        "video/quicktime" => ("video", ".mov"),
        "video/webm" => ("video", ".webm"),
        "video/x-matroska" => ("video", ".mkv"),
        "video/x-m4v" => ("video", ".m4v"),
        "video/x-msvideo" => ("video", ".avi"),
        "application/pdf" => ("pdf", ".pdf"),
        _ => return None,
    })
}

/// TS `ensureMediaAttachmentPath`：上传附件（`zcode-artifact://<session>/…`）派生一份可被工具读取的
/// 本地副本 `<root>/<kind>-cache/<session>/<kind>-<sha256(uri)[..32]><ext>`，写入原始字节。
/// 不支持的 MIME 或未配置根目录时返回 `None`（TS `unsupported`：不附路径，不阻断请求）。
pub(super) async fn derived_media_path(
    uri: &str,
    mime: &str,
    bytes: &[u8],
) -> std::io::Result<Option<String>> {
    use sha2::{Digest, Sha256};
    let (Some(root), Some((kind, extension))) = (MEDIA_CACHE_ROOT.get(), derived_extension(mime))
    else {
        return Ok(None);
    };
    let Some(session) = uri
        .strip_prefix("zcode-artifact://")
        .and_then(|rest| rest.split('/').next())
    else {
        return Ok(None);
    };
    // TS sanitizePathSegment：非 [A-Za-z0-9._-] 替换为 `_`，最长 120，空串为 unknown。
    let mut segment: String = session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .take(120)
        .collect();
    if segment.is_empty() {
        segment = "unknown".into();
    }
    let hash = Sha256::digest(uri.as_bytes());
    let name: String = hash.iter().map(|b| format!("{b:02x}")).collect::<String>()[..32].to_owned();
    let dir = root.join(format!("{kind}-cache")).join(segment);
    let path = dir.join(format!("{kind}-{name}{extension}"));
    if !tokio::fs::try_exists(&path).await? {
        tokio::fs::create_dir_all(&dir).await?;
        let temporary = dir.join(format!("{kind}-{name}{extension}.tmp-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, bytes).await?;
        if let Err(error) = tokio::fs::rename(&temporary, &path).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            // 并发物化同一派生文件时以已落盘的为准。
            if !tokio::fs::try_exists(&path).await? {
                return Err(error);
            }
        }
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}
