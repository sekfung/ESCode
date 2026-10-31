//! TS `formatReadTextOutput` 与 token 部分视图（docs/specs/rust-file-tool-results.md）。
use escode_cli_host::text_file::ReadText;

/// TS `formatReadTextOutput`：可选部分视图提示 + 逐行编号。
pub(crate) fn format_model(read: &ReadText) -> String {
    let prefix = read
        .partial_view_notice
        .as_deref()
        .map(|notice| format!("<system-reminder>{notice}</system-reminder>\n\n"))
        .unwrap_or_default();
    if read.content.is_empty() {
        let body = if read.total_lines == 0 {
            "Warning: the file exists but the contents are empty.".to_owned()
        } else {
            format!(
                "Warning: the file exists but is shorter than the provided offset ({}). The file has {} lines.",
                read.start_line, read.total_lines
            )
        };
        return format!("{prefix}<system-reminder>{body}</system-reminder>");
    }
    let numbered = read
        .content
        .split('\n')
        .enumerate()
        .map(|(index, line)| format!("{}\t{line}", read.start_line + index as u64))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{prefix}{numbered}")
}
