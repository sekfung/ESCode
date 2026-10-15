use super::*;

#[test]
fn parse_reads_first_name_and_description_lines() {
    let content = "name: My Style\ndescription: Does things\n\nbody";
    assert_eq!(
        parse_style(content, "x.md"),
        ("My Style".to_owned(), "Does things".to_owned())
    );
    assert_eq!(parse_style("no front matter", "fallback.md").0, "fallback");
    // JS `^name:\s*(.+)$`：`\s*` 可跨行，`name:` 后只有空白时下一行成为名字（TS 差分用例 gap.md 同）。
    assert_eq!(
        parse_style("name:   \ndescription: d", ".md").0,
        "description: d"
    );
    assert_eq!(parse_style("name:   ", ".md").0, "Custom Style");
    // CRLF 文件：JS 的 `.` 不含 \r，结果相同。
    assert_eq!(
        parse_style("name: Crlf\r\ndescription: D\r\n", "x.md"),
        ("Crlf".to_owned(), "D".to_owned())
    );
    // 行首必须就是 key:（`  name:` 不匹配）。
    assert_eq!(parse_style("  name: Indented\n", "x.md").0, "x");
}

#[test]
fn style_ids_replace_only_the_first_md_and_files_follow_the_prefix_rule() {
    let dirs = Dirs::new(Path::new("/h"));
    assert_eq!(
        style_file(&dirs, "custom-foo"),
        PathBuf::from("/h/.claude/output-styles/foo.md")
    );
    assert_eq!(
        style_file(&dirs, "bar"),
        PathBuf::from("/h/.claude/output-styles/bar.md")
    );
}
