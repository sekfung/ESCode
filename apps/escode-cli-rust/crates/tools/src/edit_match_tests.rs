use super::super::edit_quotes::preserve_quote_style;
use super::*;

fn matched(content: &str, search: &str, replace_all: bool) -> (String, &'static str, usize) {
    match find(content, search, replace_all) {
        MatchResult::Matched {
            actual,
            strategy,
            candidates,
        } => (actual, strategy.as_str(), candidates),
        other => panic!("expected match, got {other:?}"),
    }
}

#[test]
fn exact_wins_and_counts_non_overlapping() {
    assert_eq!(matched("aaaa", "aa", true), ("aa".into(), "exact", 2));
}

#[test]
fn quote_normalized_maps_back_to_file_text() {
    let content = "say \u{201c}hi\u{201d} 世界";
    assert_eq!(
        matched(content, "say \"hi\" 世界", false),
        ("say \u{201c}hi\u{201d} 世界".into(), "quote_normalized", 1)
    );
}

#[test]
fn line_number_prefixes_are_stripped() {
    assert_eq!(
        matched("a\nb\nc", "2: b\n3\tc", false).1,
        "line_number_prefix_stripped"
    );
    assert_eq!(
        find("a\nb", "2: b\nno prefix", false),
        MatchResult::NotFound
    );
}

#[test]
fn escape_and_unicode_escape() {
    assert_eq!(matched("a\tb", "a\\tb", false).1, "escape_normalized");
    assert_eq!(
        matched("é", "\\u00e9", false).1,
        "unicode_escape_normalized"
    );
    assert_eq!(matched("😀", "\\ud83d\\ude00", false).0, "😀");
}

#[test]
fn broad_strategies() {
    assert_eq!(
        matched("  a  \n b\n", "a\nb", false),
        ("  a  \n b".into(), "line_trimmed", 1)
    );
    // 与 TS 相同：能被 indentation_flexible 命中的块必然先被 line_trimmed 命中，这里直接验证策略本身。
    assert_eq!(
        collect(
            Strategy::IndentationFlexible,
            "\t\tif x {\n\t\t\ty\n",
            "if x {\n\ty"
        ),
        vec!["\t\tif x {\n\t\t\ty".to_owned()]
    );
    assert_eq!(
        matched(
            "start\n  middle line one\nend",
            "start\nmiddle line onx\nend",
            false
        )
        .1,
        "block_anchor"
    );
    assert_eq!(
        find("  a  \n b\n", "a\nb", true),
        MatchResult::NotFound,
        "replace_all skips broad strategies"
    );
}

#[test]
fn distinct_candidates_are_ambiguous() {
    assert_eq!(
        find(" a\nb\n  a\nb", "a \nb", false),
        MatchResult::Ambiguous { candidates: 2 }
    );
}

#[test]
fn quote_style_follows_file() {
    assert_eq!(
        preserve_quote_style("\"x\"", "\u{201c}x\u{201d}", "say \"y\" it's"),
        "say \u{201c}y\u{201d} it's"
    );
    assert_eq!(
        preserve_quote_style("'x'", "\u{2018}x\u{2019}", "'y' it's"),
        "\u{2018}y\u{2019} it\u{2019}s"
    );
}
