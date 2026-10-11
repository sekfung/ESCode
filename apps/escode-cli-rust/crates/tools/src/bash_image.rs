//! Bash 的图片 stdout 与 GitHub 限流提示（docs/specs/rust-bash-model-content.md），对齐 TS
//! `bash-image-output.ts`（`prepareBashImageOutput`）与 `bash-gh-rate-limit.ts`（`getGhRateLimitHint`）。
use crate::domain::bash_hints;
use base64::Engine as _;
use serde_json::Value;
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use escode_cli_host::image_budget::{Budget, detect, prepare_within};

const MAX_IMAGE_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// TS `READ_IMAGE_MAX_DIMENSION`；resizeToFit 只按最长边缩放，不设字节预算。
const RESIZE: Budget = Budget {
    max_base64: usize::MAX,
    max_raw: usize::MAX,
    max_tokens: None,
    max_dimension: 2000,
};

/// stdout 为图片 data URL 时换成（缩放后的）data URL 并标记 `isImage`。来源：已落盘时取完整文件（≤20 MiB），
/// 否则取内联 stdout。提供方错误不识别图片；解码失败而原图 magic 与声明一致时保留原图，否则按文本处理。
pub(crate) async fn apply_image(data: &mut Value, output_file: Option<&Path>) {
    let inline = data["stdout"].as_str().unwrap_or_default().to_owned();
    let source = match output_file {
        Some(path) => match tokio::fs::metadata(path).await {
            Ok(meta) if meta.len() <= MAX_IMAGE_FILE_BYTES => tokio::fs::read_to_string(path).await.unwrap_or(inline),
            _ => inline,
        },
        None => inline,
    };
    let Some((media, payload)) = bash_hints::image_data_url(&source) else {
        return;
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&payload) else {
        return;
    };
    if bytes.is_empty() {
        return;
    }
    let declared = media.clone();
    let original = bytes.clone();
    let resized = tokio::task::spawn_blocking(move || prepare_within(bytes, &declared, &RESIZE)).await;
    let url = match resized {
        Ok(Ok(prepared)) => format!(
            "data:{};base64,{}",
            prepared.media_type,
            base64::engine::general_purpose::STANDARD.encode(&prepared.data)
        ),
        _ if detect(&original) == Some(media.as_str()) => format!("data:{media};base64,{payload}"),
        _ => return,
    };
    data["stdout"] = url.into();
    data["isImage"] = true.into();
}

/// TS `getGhRateLimitHint`：全进程 60s 冷却，冷却期内不重复提示。
pub(crate) fn apply_gh_hint(data: &mut Value, command: &str) {
    static NEXT_AT: AtomicU64 = AtomicU64::new(0);
    let stdout = data["stdout"].as_str().unwrap_or_default();
    if !bash_hints::gh_rate_limited(command, stdout) {
        return;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let next = NEXT_AT.load(Ordering::SeqCst);
    if now < next
        || NEXT_AT
            .compare_exchange(next, now + bash_hints::GH_RATE_LIMIT_COOLDOWN_MS, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }
    data["ghRateLimitHint"] = bash_hints::GH_RATE_LIMIT_HINT.into();
}
