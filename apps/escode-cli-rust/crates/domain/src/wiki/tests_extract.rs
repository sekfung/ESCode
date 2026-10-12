use super::ir::*;
use super::*;

fn ir(blocks: Vec<Block>, format: &str) -> DocumentIr {
    DocumentIr {
        document_id: "doc-x".into(),
        source_path: "docs/Engineering Guide.md".into(),
        source_format: format.into(),
        content_hash: "sha256:1".into(),
        document_title: Some("Engineering Guide".into()),
        document_metadata: Default::default(),
        blocks,
        parser_name: "t".into(),
        parser_version: "1".into(),
        parse_status: ParseStatus::Ok,
        warnings: vec![],
        confidence: None,
    }
}

fn block(order: usize, t: BlockType, path: &[&str], loc: (&str, &str), text: &str) -> Block {
    Block {
        order,
        block_type: t,
        heading_path: path.iter().map(|s| s.to_string()).collect(),
        level: None,
        locator: ir::locator(loc.0, loc.1),
        text: text.into(),
        raw_ref: None,
        lang: None,
    }
}

#[test]
fn extraction_groups_sections_with_stable_ids() {
    let blocks = vec![
        block(0, BlockType::Heading, &["API"], ("lines", "1"), "API"),
        block(
            1,
            BlockType::Heading,
            &["API", "Error Handling"],
            ("lines", "3"),
            "Error Handling",
        ),
        block(
            2,
            BlockType::Paragraph,
            &["API", "Error Handling"],
            ("lines", "4-5"),
            "Handlers must return the standard error envelope.",
        ),
        block(
            3,
            BlockType::Code,
            &["API", "Error Handling"],
            ("lines", "6-8"),
            "{\"error\": {}}",
        ),
        block(
            4,
            BlockType::Heading,
            &["Deploy"],
            ("lines", "10"),
            "Deploy",
        ),
        block(
            5,
            BlockType::ListItem,
            &["Deploy"],
            ("lines", "11"),
            "Run the build step before deploy",
        ),
    ];
    let first = extract::extract(&ir(blocks.clone(), "markdown"));
    assert_eq!(first.len(), 2, "heading-only API section skipped");
    assert_eq!(first[0].title, "Engineering Guide — API / Error Handling");
    assert_eq!(first[0].kind, entry::Kind::Standard);
    assert_eq!(first[0].locator.value, "API / Error Handling (lines 3-8)");
    assert!(first[0].body.contains("```"));
    assert_eq!(first[1].kind, entry::Kind::Guide);
    // 章节内容变化、行号漂移：ID 不变（幂等 + 增量对上原条目）。
    let mut edited = blocks;
    edited[2].text = "Handlers must return the v2 error envelope.".into();
    edited[2].locator = ir::locator("lines", "14-15");
    let second = extract::extract(&ir(edited, "markdown"));
    assert_eq!(first[0].id, second[0].id);
    assert!(entry::valid_id(&first[0].id), "{}", first[0].id);
}

#[test]
fn structured_sources_default_to_reference_and_keep_locators() {
    let blocks = vec![block(
        0,
        BlockType::Table,
        &["Fields", "A1:C3"],
        ("cells", "Sheet: Fields → A1:C3"),
        "| name | type |\n| id | int |",
    )];
    let out = extract::extract(&ir(blocks, "xlsx"));
    assert_eq!(out[0].kind, entry::Kind::Reference);
    assert_eq!(out[0].locator.value, "Sheet: Fields → A1:C3");
    let dup = vec![
        block(
            0,
            BlockType::Paragraph,
            &["Notes"],
            ("paragraph", "1"),
            "first paragraph text long enough",
        ),
        block(
            1,
            BlockType::Heading,
            &["Other"],
            ("paragraph", "2"),
            "Other",
        ),
        block(
            2,
            BlockType::Paragraph,
            &["Notes"],
            ("paragraph", "3"),
            "second paragraph text long enough",
        ),
    ];
    let out = extract::extract(&ir(dup, "docx"));
    assert_eq!(out.len(), 2);
    assert_ne!(out[0].id, out[1].id, "repeated headings get distinct ids");
}
