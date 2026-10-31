use super::storage::Store;
use crate::domain::session::StoredAttachment;
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MEDIA_LIMIT: u64 = 20 * 1024 * 1024;
const VIDEO_LIMIT: u64 = 30 * 1024 * 1024;

impl Store {
    /// 本地路径附件：按 TS `resolveTurnAttachment` 的分支决定快照、文本预览或路径引用。
    pub(super) async fn prepare_local(&self, path: &str, mime: &str) -> Result<StoredAttachment> {
        let path = normalize_path(path)?;
        let before = tokio::fs::metadata(&path).await?;
        ensure!(before.is_file(), "Attachment must be a regular file");
        let size = before.len();
        let mime_lower = mime.to_ascii_lowercase();
        if mime_lower.starts_with("image/") {
            if size > MEDIA_LIMIT {
                return Ok(self.reference(&path, mime, size, "image_too_large"));
            }
            return self.snapshot_file(&path.to_string_lossy(), mime).await;
        }
        if mime_lower == "application/pdf" {
            if size > MEDIA_LIMIT {
                return Ok(self.reference(&path, mime, size, "pdf_too_large"));
            }
            return self.snapshot_file(&path.to_string_lossy(), mime).await;
        }
        if mime_lower.starts_with("video/") {
            if size > VIDEO_LIMIT {
                return Ok(self.reference(&path, mime, size, "video_too_large"));
            }
            return self.snapshot_file(&path.to_string_lossy(), mime).await;
        }
        if escode_cli_host::text_file::is_text_like_path(&path.to_string_lossy()) {
            return self.snapshot_text_preview(&path, mime, size).await;
        }
        Ok(self.reference(&path, mime, size, "binary_file"))
    }

    /// 不复制正文的路径引用资产（`referenceReason` 由模型请求转成 TS 文案）。
    fn reference(
        &self,
        path: &std::path::Path,
        mime: &str,
        size: u64,
        reason: &str,
    ) -> StoredAttachment {
        StoredAttachment {
            path: path.to_string_lossy().into_owned(),
            source_path: Some(path.to_string_lossy().into_owned()),
            media_type: mime.into(),
            total_bytes: size,
            original_bytes: None,
            reference_reason: Some(reason.into()),
            preview_total_lines: None,
            preview_notice: None,
        }
    }

    /// 文本附件：TS `readTextFileForModel` 的有界预览。
    /// 超过 `READ_MAX_FILE_SIZE_BYTES` 时 TS 显式传 `limit=READ_DEFAULT_MAX_LINES, offset=1`；
    /// 未超限时保持整文件读取（无行数上限），不能一律按 2000 行截断。
    async fn snapshot_text_preview(
        &self,
        path: &std::path::Path,
        mime: &str,
        size: u64,
    ) -> Result<StoredAttachment> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let oversized = size > escode_cli_host::text_file::READ_MAX_FILE_SIZE_BYTES;
        let (offset, limit) = if oversized {
            (
                Some(1),
                Some(escode_cli_host::text_file::READ_DEFAULT_MAX_LINES),
            )
        } else {
            (None, None)
        };
        let read = escode_cli_host::text_file::read(path, offset, limit, true, &cancel).await?;
        let mut asset = self
            .save_attachment_with_limit(&[read.content.clone().into_bytes()], mime, MEDIA_LIMIT)
            .await?;
        asset.source_path = Some(path.to_string_lossy().into_owned());
        asset.original_bytes = Some(size);
        asset.preview_total_lines = Some(read.total_lines);
        asset.preview_notice = read.partial_view_notice;
        Ok(asset)
    }

    pub(super) async fn snapshot_file(&self, path: &str, mime: &str) -> Result<StoredAttachment> {
        let path = normalize_path(path)?;
        let max = if mime.to_ascii_lowercase().starts_with("video/") {
            VIDEO_LIMIT
        } else {
            MEDIA_LIMIT
        };
        let mut file = tokio::fs::File::open(&path).await?;
        let before = file.metadata().await?;
        ensure!(
            before.is_file() && before.len() <= max,
            "Attachment must be a regular file within {} MiB",
            max / (1024 * 1024)
        );
        let mut chunks = Vec::new();
        let mut total = 0;
        loop {
            let mut chunk = vec![0; 512 * 1024];
            let count = file.read(&mut chunk).await?;
            if count == 0 {
                break;
            }
            total += count as u64;
            ensure!(total <= max, "Attachment grew beyond size limit");
            chunk.truncate(count);
            chunks.push(chunk);
        }
        let after = file.metadata().await?;
        // 同一次 admission 不能把变更中的文件伪装成稳定快照；排队提升只读取已提交副本。
        ensure!(
            total == before.len()
                && after.len() == before.len()
                && after.modified()? == before.modified()?,
            "Attachment changed during snapshot"
        );
        let mut asset = self.save_attachment_with_limit(&chunks, mime, max).await?;
        asset.source_path = Some(path.to_string_lossy().into_owned());
        Ok(asset)
    }

    pub(super) async fn save_attachment(
        &self,
        chunks: &[Vec<u8>],
        mime: &str,
    ) -> Result<StoredAttachment> {
        self.save_attachment_with_limit(chunks, mime, MEDIA_LIMIT)
            .await
    }

    async fn save_attachment_with_limit(
        &self,
        chunks: &[Vec<u8>],
        mime: &str,
        max: u64,
    ) -> Result<StoredAttachment> {
        let total: usize = chunks.iter().map(Vec::len).sum();
        ensure!(total as u64 <= max, "Attachment exceeds size limit");
        let mut hash = Sha256::new();
        for chunk in chunks {
            hash.update(chunk);
        }
        let path = self.attachment_root.join(format!("{:x}", hash.finalize()));
        tokio::fs::create_dir_all(&self.attachment_root).await?;
        let temp = self.attachment_root.join(super::id());
        let write = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temp).await?;
            for chunk in chunks {
                file.write_all(chunk).await?;
            }
            file.sync_all().await?;
            drop(file);
            // 内容寻址且不可变，已有同一内容可复用；不会覆盖正在被历史引用的不同字节。
            if tokio::fs::try_exists(&path).await? {
                tokio::fs::remove_file(&temp).await?;
            } else {
                tokio::fs::rename(&temp, &path).await?;
            }
            #[cfg(unix)]
            tokio::fs::File::open(&self.attachment_root)
                .await?
                .sync_all()
                .await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if write.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        write?;
        Ok(StoredAttachment {
            path: path.to_string_lossy().into_owned(),
            source_path: None,
            media_type: mime.into(),
            total_bytes: total as u64,
            original_bytes: None,
            reference_reason: None,
            preview_total_lines: None,
            preview_notice: None,
        })
    }
}

fn normalize_path(path: &str) -> Result<std::path::PathBuf> {
    if path.starts_with("file://") {
        return reqwest::Url::parse(path)?
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("Invalid attachment file URL"));
    }
    Ok(path.into())
}
