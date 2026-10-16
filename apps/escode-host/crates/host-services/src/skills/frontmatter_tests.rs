use super::*;

#[test]
fn headings_and_fences() {
    assert!(is_heading_line("# Title") && is_heading_line("###### x") && is_heading_line("#\tx"));
    assert!(
        !is_heading_line("####### x")
            && !is_heading_line("#x")
            && !is_heading_line("# ")
            && !is_heading_line("x # y")
    );
    let text = "---\nname: a\n---\nbody";
    assert_eq!(split_frontmatter(text), Some(("name: a", text.len() - 4)));
    assert_eq!(split_frontmatter("---\n---\nbody"), None);
    assert_eq!(
        split_frontmatter("---\nname: a\n---x\n---"),
        Some(("name: a\n---x", 20))
    );
}

#[test]
fn loose_inline_strips_one_quote_per_end() {
    assert_eq!(loose_inline("\"a\""), "a");
    assert_eq!(loose_inline("'a"), "a");
    assert_eq!(loose_inline("\""), "");
    assert_eq!(loose_inline("\"\"a\"\""), "\"a\"");
}

/// TS 真实 `readFrontmatter` 导出的黄金样本（`scripts/generate-escode-host-skills-fixtures.mjs`）。
#[test]
fn frontmatter_matches_the_ts_reader() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("frontmatter_golden.json")).expect("golden");
    for case in golden["cases"].as_array().expect("cases") {
        let text = case["text"].as_str().expect("text");
        let expected = &case["expected"];
        let actual = read(text);
        let actual_json = serde_json::json!({
            "hasFrontmatter": actual.has_frontmatter, "name": actual.name, "description": actual.description,
            "body": actual.body, "keys": actual.keys, "parseOk": actual.parse_ok,
        });
        assert_eq!(&actual_json, expected, "case {text:?}");
    }
}
