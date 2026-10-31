use super::mcp_result::{decimal_byte_size, persisted_output_envelope};

#[test]
fn decimal_byte_size_matches_node_format_decimal_bytes() {
    // TS formatDecimalBytes：B 原样，其余 1000 进制四舍五入。
    assert_eq!(decimal_byte_size(0), "0 B");
    assert_eq!(decimal_byte_size(999), "999 B");
    assert_eq!(decimal_byte_size(1_000), "1 KB");
    assert_eq!(decimal_byte_size(1_500), "2 KB");
    assert_eq!(decimal_byte_size(999_500), "1000 KB");
    assert_eq!(decimal_byte_size(1_000_000), "1 MB");
    assert_eq!(decimal_byte_size(1_500_000_000), "2 GB");
}

#[test]
fn persisted_envelope_omits_ellipsis_for_short_content() {
    let content = "short output";
    assert_eq!(
        persisted_output_envelope(content, 12, "/o/x.json"),
        "<persisted-output>\nOutput too large (12 B). Full output saved to: /o/x.json\n\nPreview (first 2 KB):\nshort output\n</persisted-output>"
    );
}

#[test]
fn persisted_envelope_trims_preview_at_newline_after_half() {
    // 前 2 000 字符内的最后一个换行位于后半段 → 在换行处截断并补省略号。
    let content = format!("{}\n{}", "a".repeat(1_500), "b".repeat(1_000));
    let envelope = persisted_output_envelope(&content, content.len(), "/o/x.json");
    assert!(envelope.contains(&format!(
        "\n\nPreview (first 2 KB):\n{}\n...\n",
        "a".repeat(1_500)
    )));
}

#[test]
fn persisted_envelope_keeps_full_preview_when_newline_is_in_first_half() {
    let content = format!("{}\n{}", "a".repeat(500), "b".repeat(2_000));
    let envelope = persisted_output_envelope(&content, content.len(), "/o/x.json");
    let preview = format!("{}\n{}", "a".repeat(500), "b".repeat(1_499));
    assert!(envelope.contains(&format!("\n\nPreview (first 2 KB):\n{preview}\n...\n")));
}

#[test]
fn persisted_envelope_counts_preview_in_utf16_units() {
    // 非 BMP 字符占 2 个 UTF-16 单元，与 TS `content.length` 同口径。
    let content = "😀".repeat(1_500);
    let envelope = persisted_output_envelope(&content, content.len(), "/o/x.json");
    assert!(envelope.contains(&format!(
        "\n\nPreview (first 2 KB):\n{}\n...\n",
        "😀".repeat(1_000)
    )));
}
