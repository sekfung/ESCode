//! 验收 §15.3（Office / PDF / 旧格式）：用真实生成的 DOCX/XLSX/PPTX/PDF 文件验证结构与定位。
mod common;
use common::*;
use escode_cli_domain::wiki::config::WikiConfig;
use escode_cli_domain::wiki::entry::Scope;
use escode_cli_domain::wiki::ir::BlockType;
use escode_cli_wiki::ingest::{Ctx, ParseResult, parse};
use std::time::{Duration, Instant};

fn run(format: &str, bytes: &[u8]) -> ParseResult {
    let config = WikiConfig::default();
    let ctx = Ctx {
        limits: &config.limits,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    parse(format, bytes, &ctx)
}

#[test]
fn docx_headings_lists_tables_links_footnotes() {
    let body = [
        para(Some("Title"), "Ignored Title Paragraph"),
        para(Some("1"), "API"),
        para(Some("Heading2"), "Error Handling"),
        "<w:p><w:r><w:t>Handlers must use the </w:t></w:r><w:hyperlink r:id=\"rId9\"><w:r><w:t>error format</w:t></w:r></w:hyperlink><w:r><w:t> &amp; log it.</w:t></w:r><w:r><w:footnoteReference w:id=\"2\"/></w:r></w:p>".to_owned(),
        "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>nested item</w:t></w:r></w:p>".to_owned(),
        "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Code</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Meaning</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>E1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Bad | input</w:t></w:r></w:p></w:tc></w:tr></w:tbl>".to_owned(),
        "<w:p><w:r><w:drawing/></w:r></w:p>".to_owned(),
    ]
    .join("");
    let p = run("docx", &docx(&body)).unwrap();
    assert_eq!(
        p.title.as_deref(),
        Some("Engineering Standards"),
        "core title wins"
    );
    assert_eq!(p.metadata["creator"], "QA");
    let para = p
        .blocks
        .iter()
        .find(|b| b.text.starts_with("Handlers"))
        .unwrap();
    assert_eq!(
        para.heading_path,
        vec!["API", "Error Handling"],
        "localized style id '1' resolved via styles.xml"
    );
    assert!(
        para.text
            .contains("[error format](https://example.com/errors)")
            && para.text.contains(" & log it.[^2]"),
        "{}",
        para.text
    );
    let item = p
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::ListItem)
        .unwrap();
    assert_eq!(item.text, "  nested item");
    let table = p
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Table)
        .unwrap();
    assert!(
        table.text.contains("| Code | Meaning |") && table.text.contains("Bad \\| input"),
        "{}",
        table.text
    );
    assert_eq!(table.locator.value, "table 1 (2 rows × 2 columns)");
    let note = p
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Footnote)
        .unwrap();
    assert!(note.text.contains("RFC 7807") && note.locator.value.ends_with("#2"));
    assert!(
        !p.blocks.iter().any(|b| b.text == "sep"),
        "separator footnote skipped"
    );
    assert!(p.warnings.iter().any(|w| w.contains("1 images")));
}

#[test]
fn xlsx_sheets_formulas_merges_hidden_and_headers() {
    let fields = "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>\
        <row r=\"2\"><c r=\"A2\" t=\"s\"><v>2</v></c><c r=\"B2\"><v>10</v></c></row>\
        <row r=\"3\" hidden=\"1\"><c r=\"A3\" t=\"inlineStr\"><is><t>rate</t></is></c><c r=\"B3\"><f>B2*2</f><v>20</v></c></row>\
        <row r=\"4\"><c r=\"A4\" t=\"s\"><v>0</v></c><c r=\"B4\" t=\"s\"><v>1</v></c></row>\
        <row r=\"5\"><c r=\"A5\" t=\"str\"><v>total</v></c><c r=\"B5\"><f t=\"shared\" si=\"0\">SUM(B2:B3)</f><v>30</v></c></row>\
        <row r=\"6\"><c r=\"A6\" t=\"str\"><v>total2</v></c><c r=\"B6\"><f t=\"shared\" si=\"0\"/></c></row>\
        <row r=\"9\"><c r=\"D9\" t=\"b\"><v>1</v></c><c r=\"E9\" t=\"e\"><v>#DIV/0!</v></c></row>";
    let bytes = xlsx(
        &["field", "value", "limit"],
        &[
            (
                "Fields",
                false,
                fields,
                "<mergeCells><mergeCell ref=\"A1:B1\"/></mergeCells>",
            ),
            (
                "Secret",
                true,
                "<row r=\"1\"><c r=\"A1\" t=\"str\"><v>internal</v></c></row>",
                "",
            ),
        ],
    );
    let p = run("xlsx", &bytes).unwrap();
    assert_eq!(
        p.metadata["sheets"],
        serde_json::json!(["Fields", "Secret"])
    );
    assert_eq!(
        p.metadata["definedNames"],
        serde_json::json!(["Rates = Fields!$B$2:$B$3"])
    );
    let first = &p.blocks[0];
    assert_eq!(first.heading_path, vec!["Fields", "Table 1"]);
    assert_eq!(first.locator.value, "Sheet: Fields → A1:B6");
    let text = &first.text;
    assert!(
        text.contains("Merged cells: A1:B1") && text.contains("Hidden rows: 3"),
        "{text}"
    );
    assert!(
        text.contains("=B2*2 → 20 (cached)"),
        "formula vs cached value: {text}"
    );
    assert!(
        text.contains("=SUM(B2:B3) (shared formula, derived) → (no cached value)"),
        "{text}"
    );
    assert!(
        !text.contains("| 4 |"),
        "repeated header row skipped: {text}"
    );
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("row 4 repeats the header"))
    );
    let second = &p.blocks[1];
    assert_eq!(
        second.locator.value, "Sheet: Fields → D9:E9",
        "blank row splits regions"
    );
    assert!(second.text.contains("TRUE") && second.text.contains("#ERROR #DIV/0!"));
    assert!(
        p.blocks
            .iter()
            .any(|b| b.heading_path[0] == "Secret (hidden)")
    );
    let mut tiny = WikiConfig::default();
    tiny.limits.max_sheet_cells = 3;
    let ctx = Ctx {
        limits: &tiny.limits,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    let truncated = parse("xlsx", &bytes, &ctx).unwrap();
    assert!(
        truncated.degraded
            && truncated
                .warnings
                .iter()
                .any(|w| w.contains("cell limit 3"))
    );
}

#[test]
fn pptx_slides_titles_and_notes() {
    let p = run(
        "pptx",
        &pptx(&[
            (
                "Architecture",
                "Gateway routes all traffic",
                Some("mention SLOs"),
            ),
            ("Decisions", "We chose Postgres", None),
        ]),
    )
    .unwrap();
    assert_eq!(p.metadata["slides"], 2);
    assert!(
        p.blocks
            .iter()
            .any(|b| b.heading_path == vec!["Slide 1: Architecture"] && b.text.contains("Gateway"))
    );
    let note = p
        .blocks
        .iter()
        .find(|b| b.block_type == BlockType::Note)
        .unwrap();
    assert!(note.text.contains("mention SLOs") && note.locator.value == "slide 1 notes");
    assert!(p.warnings.iter().any(|w| w.contains("pictures")));
}

#[test]
fn pdf_pages_and_scanned_detection() {
    let bytes = pdf(&[
        &[
            "SECURITY GUIDE",
            "Passwords must be hashed with argon2id before storage.",
        ],
        &[],
        &["Tokens expire after fifteen minutes in every service."],
    ]);
    let p = run("pdf", &bytes).unwrap();
    assert_eq!(p.metadata["pages"], 3);
    let hashing = p
        .blocks
        .iter()
        .find(|b| b.text.contains("argon2id"))
        .unwrap();
    assert_eq!(hashing.locator.value, "1");
    assert_eq!(hashing.heading_path, vec!["SECURITY GUIDE"]);
    assert!(
        p.blocks
            .iter()
            .any(|b| b.text.contains("fifteen") && b.locator.value == "3")
    );
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("without a text layer") && w.contains('2'))
    );
    let scanned = run("pdf", &pdf(&[&[], &[]])).unwrap();
    assert!(
        scanned.degraded
            && scanned
                .warnings
                .iter()
                .any(|w| w.starts_with("ocr_unavailable"))
    );
}

#[test]
fn zip_bomb_and_macros() {
    let big = "A".repeat(3 * 1024 * 1024);
    let bomb = zip_bytes(&[
        ("word/document.xml", &big),
        ("[Content_Types].xml", "<Types/>"),
    ]);
    assert_eq!(run("docx", &bomb).unwrap_err().stage, "security");
    let body = para(None, "text");
    let mut parts = vec![];
    let document = format!("<w:document xmlns:w=\"w\"><w:body>{body}</w:body></w:document>");
    parts.push(("word/document.xml", document.as_str()));
    parts.push(("word/vbaProject.bin", "MACRO"));
    let p = run("docx", &zip_bytes(&parts)).unwrap();
    assert!(p.warnings.iter().any(|w| w.contains("macros")));
}

#[tokio::test]
async fn office_documents_end_to_end_and_legacy_converter() {
    let env = Env::new();
    let repo = env.repo("docs", Some("https://h/o/docs"));
    let docx_path = repo.join("specs/api.docx");
    std::fs::create_dir_all(docx_path.parent().unwrap()).unwrap();
    std::fs::write(
        &docx_path,
        docx(
            &[
                para(Some("1"), "API"),
                para(Some("Heading2"), "Errors"),
                para(None, "Handlers must return problem+json responses."),
            ]
            .join(""),
        ),
    )
    .unwrap();
    std::fs::write(
        repo.join("specs/legacy.doc"),
        [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0],
    )
    .unwrap();
    let wiki = env.service(&repo);
    let report = wiki
        .ingest(&[repo.join("specs")], Scope::Project, false)
        .await
        .unwrap();
    let docx_item = report
        .items
        .iter()
        .find(|i| i.path.ends_with("api.docx"))
        .unwrap();
    assert_eq!(docx_item.status, "imported");
    let legacy = report
        .items
        .iter()
        .find(|i| i.path.ends_with("legacy.doc"))
        .unwrap();
    assert_eq!(legacy.status, "failed");
    assert!(
        legacy
            .error
            .as_deref()
            .unwrap()
            .contains("converter_unavailable"),
        "{legacy:?}"
    );
    let read = wiki
        .tool(
            "WikiSearch",
            &serde_json::json!({"query": "problem json responses", "include_proposals": true}),
            "s",
        )
        .await
        .unwrap();
    assert!(
        read.content.contains("api.docx → API / Errors"),
        "{}",
        read.content
    );
    // 可选转换器：用户层配置一个假的 soffice（把预置的 docx 拷贝为输出），验证调用链与降级说明。
    #[cfg(unix)]
    {
        let fixture = env.work.path().join("converted.docx");
        std::fs::write(
            &fixture,
            docx(&para(
                None,
                "Converted legacy rule: retries must be bounded.",
            )),
        )
        .unwrap();
        let script = env.work.path().join("fake-soffice");
        // 修复：由独立子进程写入并 chmod，避免并发用例 fork 继承本进程的写句柄导致执行时 ETXTBSY。
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"")
            .arg("sh")
            .arg(format!("#!/bin/sh\n# args: --headless --norestore --convert-to docx --outdir DIR FILE\ncp '{}' \"$6/source.docx\"\n", fixture.display()))
            .arg(&script)
            .status()
            .unwrap();
        assert!(status.success(), "write script: {status}");
        env.user_config(serde_json::json!({"wiki": {"converters": {"office": {"enabled": true, "command": script.to_string_lossy()}}}}));
        let again = wiki
            .ingest(&[repo.join("specs/legacy.doc")], Scope::Project, false)
            .await
            .unwrap();
        assert_eq!(again.items[0].status, "updated", "{again:?}");
        assert!(
            again.items[0]
                .warnings
                .iter()
                .any(|w| w.contains("converted from legacy doc"))
        );
    }
}
