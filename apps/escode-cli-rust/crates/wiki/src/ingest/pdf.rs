//! 文本型 PDF（spec §7.6）：逐页抽取文本（lopdf），保留页码；标题按启发式识别（短行、无句末标点、
//! 编号或全大写）并明确告警。无文本层的页面视为扫描页：全部无文本时为 degraded（OCR 本期未实现），不编造内容。
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};

fn looks_like_heading(line: &str, next: Option<&str>) -> bool {
    let t = line.trim();
    let chars = t.chars().count();
    if !(2..=80).contains(&chars) || t.ends_with(['.', '。', ',', '，', ';', ':', '：']) {
        return false;
    }
    let numbered = t.split_once(' ').is_some_and(|(n, rest)| {
        !rest.is_empty()
            && n.trim_end_matches('.')
                .split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    let upper = t.chars().any(char::is_alphabetic)
        && t.chars()
            .filter(|c| c.is_alphabetic())
            .all(char::is_uppercase);
    let followed_by_body = next.is_some_and(|n| n.trim().chars().count() > chars);
    numbered || (upper && chars >= 4) || (followed_by_body && chars <= 40 && !t.contains("  "))
}

pub fn parse(bytes: &[u8], ctx: &Ctx<'_>) -> ParseResult {
    let document = lopdf::Document::load_mem(bytes)
        .map_err(|e| Failure::parse(format!("cannot read PDF: {e}")))?;
    if document.is_encrypted() {
        return Err(Failure::parse("encrypted PDF is not supported"));
    }
    let pages = document.get_pages();
    if pages.len() > ctx.limits.max_pdf_pages {
        return Err(Failure::limit(format!(
            "PDF has {} pages (wiki.limits.maxPdfPages = {})",
            pages.len(),
            ctx.limits.max_pdf_pages
        )));
    }
    let mut parsed = Parsed::default();
    let mut sink = BlockSink::default();
    let mut stack: Vec<String> = vec![];
    let (mut empty_pages, mut short_lines, mut total_lines, mut headings) =
        (vec![], 0usize, 0usize, 0usize);
    for &number in pages.keys() {
        ctx.check_time()?;
        let text = document.extract_text(&[number]).unwrap_or_default();
        if text.trim().is_empty() {
            empty_pages.push(number);
            continue;
        }
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        let mut paragraph: Vec<&str> = vec![];
        let flush = |paragraph: &mut Vec<&str>, sink: &mut BlockSink, stack: &Vec<String>| {
            if paragraph.is_empty() {
                return;
            }
            let path = if stack.is_empty() {
                vec![format!("Page {number}")]
            } else {
                stack.clone()
            };
            sink.push(
                BlockType::Paragraph,
                &path,
                locator("page", number.to_string()),
                paragraph.join("\n"),
            );
            paragraph.clear();
        };
        for (i, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                flush(&mut paragraph, &mut sink, &stack);
                continue;
            }
            total_lines += 1;
            if line.trim().chars().count() < 25 {
                short_lines += 1;
            }
            if paragraph.is_empty() && looks_like_heading(line, lines.get(i + 1).copied()) {
                headings += 1;
                stack = vec![line.trim().to_owned()];
                sink.push(
                    BlockType::Heading,
                    &stack,
                    locator("page", number.to_string()),
                    line.trim(),
                )
                .level = Some(1);
                continue;
            }
            paragraph.push(line.trim());
        }
        flush(&mut paragraph, &mut sink, &stack);
    }
    if !empty_pages.is_empty() {
        let list: Vec<String> = empty_pages.iter().map(u32::to_string).collect();
        parsed.warnings.push(format!(
            "pages without a text layer (scanned images, OCR not available): {}",
            list.join(", ")
        ));
    }
    if sink.blocks.is_empty() {
        parsed.degraded = true;
        parsed.confidence = Some(0.0);
        parsed.warnings.push("ocr_unavailable: no extractable text; this looks like a scanned PDF and OCR is not implemented".into());
    }
    if headings > 0 {
        parsed
            .warnings
            .push("PDF headings were inferred heuristically from line layout".into());
    }
    if total_lines > 20 && short_lines * 2 > total_lines {
        parsed.warnings.push("many short lines: the page may use multiple columns; reading order follows the PDF content stream".into());
    }
    parsed.metadata.insert("pages".into(), pages.len().into());
    parsed.blocks = sink.blocks;
    Ok(parsed)
}
