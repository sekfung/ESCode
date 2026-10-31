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
    fn target(&self, extension: &str) -> (std::path::PathBuf, String, String) {
        let artifact = format!("tool-result-{}", escode_cli_host::id());
        let dir = self.root.join(sanitize(self.session));
        let path = dir.join(format!("{}-{artifact}{extension}", sanitize(self.call_id)));
        let uri = format!("escode-artifact://{}/{artifact}", self.session);
        (path, uri, artifact)
    }

    pub(super) async fn write(&self, mime: &str, bytes: &[u8]) -> Result<(String, String)> {
        let (path, uri, _) = self.target(crate::domain::mcp_result::image_extension(mime));
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, bytes).await?;
        Ok((path.to_string_lossy().into_owned(), uri))
    }

    /// TS `NodeToolArtifactStore.writeToolResultArtifact`：artifact 策略的完整工具结果文本
    /// （MCP 的 `output` 是结果对象，内容类型为 `application/json`，扩展名同 TS `.json`）。
    pub(super) async fn write_text(&self, content_type: &str, content: &str) -> Result<String> {
        let (path, _, _) = self.target(text_extension(content_type));
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, content.as_bytes()).await?;
        Ok(path.to_string_lossy().into_owned())
    }
}

/// TS `extensionForContentType` 的文本子集（artifact 策略只会遇到 JSON/TEXT）。
fn text_extension(content_type: &str) -> &'static str {
    match content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "text/plain" => ".txt",
        "text/markdown" => ".md",
        _ => ".json",
    }
}
