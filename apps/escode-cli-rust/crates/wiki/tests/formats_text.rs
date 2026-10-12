//! 验收 §15.3（文本类格式）与资源限制。
mod common;
use escode_cli_domain::wiki::config::WikiConfig;
use escode_cli_domain::wiki::ir::BlockType;
use escode_cli_wiki::ingest::{Ctx, ParseResult, detect, parse};
use std::time::{Duration, Instant};

fn run(format: &str, text: &str) -> ParseResult {
    run_with(format, text.as_bytes(), &WikiConfig::default())
}

fn run_with(format: &str, bytes: &[u8], config: &WikiConfig) -> ParseResult {
    let ctx = Ctx {
        limits: &config.limits,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    parse(format, bytes, &ctx)
}

#[test]
fn markdown_structure() {
    let md = "---\ntitle: Guide\nowner: qa\n---\n# Guide\n\nIntro with [link](./a.md) and ![img](img/x.png).\n\nSub Title\n---------\n\n- item one\n  continued\n- item two\n\n> quoted rule\n\n```rust\nfn main() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
    let p = run("markdown", md).unwrap();
    assert_eq!(p.title.as_deref(), Some("Guide"));
    assert_eq!(p.metadata["frontmatter"]["owner"], "qa");
    let kinds: Vec<BlockType> = p.blocks.iter().map(|b| b.block_type).collect();
    for k in [
        BlockType::Heading,
        BlockType::Paragraph,
        BlockType::ListItem,
        BlockType::Quote,
        BlockType::Code,
        BlockType::Table,
    ] {
        assert!(kinds.contains(&k), "missing {k:?}: {kinds:?}");
    }
    let code = p
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Code)
        .unwrap();
    assert_eq!(code.lang.as_deref(), Some("rust"));
    assert_eq!(code.heading_path, vec!["Guide", "Sub Title"]);
    let para = p
        .blocks
        .iter()
        .find(|b| b.text.contains("![img](img/x.png)"))
        .unwrap();
    assert_eq!(
        para.locator.value, "7-7",
        "line numbers count from the file start"
    );
    assert_eq!(
        p.blocks
            .iter()
            .filter(|b| b.block_type == BlockType::ListItem)
            .count(),
        2
    );
}

#[test]
fn text_encodings() {
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in "规范 rule".encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    let p = run_with("txt", &utf16, &WikiConfig::default()).unwrap();
    assert!(p.blocks[0].text.contains("规范"));
    let err = run_with(
        "txt",
        &[0x66, 0xFF, 0xFE, 0x41, 0x80],
        &WikiConfig::default(),
    )
    .unwrap_err();
    assert_eq!(err.stage, "decode");
    assert!(err.message.contains("offset 1"), "{err:?}");
}

#[test]
fn csv_and_tsv() {
    let csv = "name,type,,name\n\"id\",int,\"primary, key\",x\n\"note\",\"text \"\"quoted\"\"\nline2\",,y\n";
    let p = run("csv", csv).unwrap();
    assert_eq!(
        p.metadata["columns"],
        serde_json::json!(["name", "type", "column_3", "name_2"])
    );
    assert!(
        p.warnings.iter().any(|w| w.contains("empty header"))
            && p.warnings.iter().any(|w| w.contains("duplicate"))
    );
    let table = &p.blocks[0];
    assert!(
        table.text.contains("primary, key") && table.text.contains("text \"quoted\" line2"),
        "{}",
        table.text
    );
    assert_eq!(table.locator.value, "records 1-2 (lines 2-3)");
    let tsv = run("tsv", "1\t2\n3\t4\n").unwrap();
    assert!(tsv.warnings.iter().any(|w| w.contains("synthetic")));
    assert!(run("csv", "a,b\n\"unterminated,1\n").is_err());
}

#[test]
fn json_and_yaml_paths_and_errors() {
    let p = run(
        "json",
        "{\n  \"services\": {\"auth\": {\"timeout\": 30, \"hosts\": [\"a\"]}},\n  \"x y\": null\n}",
    )
    .unwrap();
    let auth = &p.blocks[0];
    assert!(
        auth.text.contains("`$.services.auth.timeout` (number): 30"),
        "{}",
        auth.text
    );
    assert!(auth.text.contains("`$.services.auth.hosts[0]` (string): a"));
    assert_eq!(auth.locator.value, "$.services (line 2)");
    assert!(p.blocks[1].text.contains("$[\"x y\"]"));
    let err = run("json", "{\n  \"a\": [1, 2,\n}").unwrap_err();
    assert!(err.message.contains("line 3"), "{err:?}");
    let y = run(
        "yaml",
        "# Timeouts in seconds\nauth:\n  timeout: 30\nflags: [a, b]\n",
    )
    .unwrap();
    assert!(
        y.blocks[0]
            .text
            .starts_with("Comments: Timeouts in seconds")
    );
    assert!(y.blocks[0].text.contains("`$.auth.timeout` (number): 30"));
    assert_eq!(y.blocks[0].locator.value, "$.auth (line 2)");
    let bad = run("yaml", "a: [1, 2\nb: c\n").unwrap_err();
    assert!(bad.message.contains("line"), "{bad:?}");
}

#[test]
fn xml_paths_and_entity_safety() {
    let p = run("xml", "<?xml version=\"1.0\"?><config><service name=\"auth\"><timeout>30</timeout></service><service name=\"db\"><note>a &amp; b</note></service><flag/></config>").unwrap();
    assert_eq!(p.title.as_deref(), Some("config"));
    assert!(
        p.blocks[0].text.contains("`/config/service/timeout`: 30"),
        "{:?}",
        p.blocks
    );
    assert!(
        p.blocks[1]
            .text
            .contains("`/config/service[2]` attributes: name=\"db\"")
    );
    assert!(p.blocks[1].text.contains("a & b"));
    let bomb = "<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;\">]><lolz>&lol2;</lolz>";
    assert_eq!(run("xml", bomb).unwrap_err().stage, "security");
    let external =
        "<?xml version=\"1.0\"?><!DOCTYPE r SYSTEM \"http://evil.example/x.dtd\"><r>&ext;</r>";
    let ok = run("xml", external).unwrap();
    assert!(ok.warnings.iter().any(|w| w.contains("never loaded")));
    assert!(!ok.blocks.iter().any(|b| b.text.contains("evil")));
}

#[test]
fn rst_structure() {
    let rst = "=====\nGuide\n=====\n\nSection\n-------\n\nText here::\n\n    code block\n\n.. note:: careful\n\n- item\n";
    let p = run("rst", rst).unwrap();
    assert_eq!(p.title.as_deref(), Some("Guide"));
    assert!(
        p.blocks
            .iter()
            .any(|b| b.block_type == BlockType::Code && b.text == "code block")
    );
    assert!(p.warnings.iter().any(|w| w.contains("directive 'note'")));
    assert!(
        p.blocks
            .iter()
            .any(|b| b.heading_path == vec!["Guide", "Section"])
    );
}

#[test]
fn identification_and_limits() {
    assert_eq!(detect("a.md", b"# x").0, Some("markdown"));
    let (format, warnings) = detect("report.docx", b"%PDF-1.5 ...");
    assert_eq!(format, Some("pdf"));
    assert!(!warnings.is_empty());
    assert_eq!(detect("fake.xlsx", b"plain text").0, None);
    let mut config = WikiConfig::default();
    config.limits.max_file_bytes = 10;
    assert_eq!(
        run_with("txt", b"0123456789ABC", &config)
            .unwrap_err()
            .stage,
        "security"
    );
    let mut deep = String::new();
    for _ in 0..80 {
        deep.push_str("{\"a\":");
    }
    deep.push('1');
    for _ in 0..80 {
        deep.push('}');
    }
    let err = run("json", &deep).unwrap_err();
    assert!(err.stage == "security" || err.stage == "parse", "{err:?}");
    let mut c = WikiConfig::default();
    c.limits.max_records = 3;
    assert_eq!(
        run_with("csv", b"a\n1\n2\n3\n4\n5\n", &c)
            .unwrap_err()
            .stage,
        "security"
    );
}
