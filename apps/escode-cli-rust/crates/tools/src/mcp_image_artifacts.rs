//! MCP 超预算图片的二进制 artifact 目标与路径清理（从 mcp_connection.rs 拆出）。

use anyhow::Result;

/// MCP 超预算图片的二进制 artifact 目标（TS `writeToolResultBinaryArtifact`）：
/// `<root>/<session>/<toolCallId>-tool-result-<uuid><ext>`，URI `escode-artifact://<session>/tool-result-<uuid>`。
#[derive(Clone, Copy)]
pub struct ImageArtifacts<'a> {
    pub root: &'a std::path::Path,
    pub session: &'a str,
    pub call_id: &'a str,
}
/// TS `sanitizePathSegment`（存储版）：非 `[A-Za-z0-9._-]` 换成 `_`，最长 120，空串为 unknown。
pub(crate) fn sanitize(value: &str) -> String {
    let out: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if out.is_empty() {
        "unknown".to_owned()
    } else {
        out
    }
}
impl ImageArtifacts<'_> {
    pub(super) async fn write(&self, mime: &str, bytes: &[u8]) -> Result<(String, String)> {
        let artifact = format!("tool-result-{}", escode_cli_host::id());
        let dir = self.root.join(sanitize(self.session));
        let path = dir.join(format!(
            "{}-{artifact}{}",
            sanitize(self.call_id),
            crate::domain::mcp_result::image_extension(mime)
        ));
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(&path, bytes).await?;
        Ok((
            path.to_string_lossy().into_owned(),
            format!("escode-artifact://{}/{artifact}", self.session),
        ))
    }
}
