//! 文件不存在时的模型文案（docs/specs/rust-file-tool-results.md，TS `createMissing{Read,Edit}FileMessage`）：
//! 列出同目录的文件与符号链接，给出相近文件名建议。
use std::path::Path;

pub(super) async fn message(cwd: &Path, path: &Path) -> String {
    let suggestion = async {
        let target = path.file_name()?.to_string_lossy().into_owned();
        let mut entries = tokio::fs::read_dir(path.parent()?).await.ok()?;
        let mut names = vec![];
        while let Ok(Some(entry)) = entries.next_entry().await {
            let kind = entry.file_type().await.ok()?;
            if kind.is_file() || kind.is_symlink() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        crate::domain::file_tool_text::similar_filename(&target, &names)
    }
    .await;
    crate::domain::file_tool_text::missing_file(&cwd.to_string_lossy(), suggestion.as_deref())
}
