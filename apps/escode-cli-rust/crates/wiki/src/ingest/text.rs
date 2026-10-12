//! 文本解码与 TXT 解析。编码：UTF-8（可带 BOM）、UTF-16 LE/BE（需 BOM）；其余编码识别失败时明确报错，
//! 不做有损猜测（spec §7.2「编码识别失败时报告」）。TXT 按空行分段、每 40 行左右成一个部分。
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};

const LINES_PER_PART: usize = 40;

pub fn decode(bytes: &[u8]) -> Result<String, Failure> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return utf8(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes);
    }
    utf8(bytes)
}

fn utf8(bytes: &[u8]) -> Result<String, Failure> {
    String::from_utf8(bytes.to_vec()).map_err(|e| {
        Failure::new(
            "decode",
            format!(
                "text is not valid UTF-8 (first invalid byte at offset {}); convert the file to UTF-8 or UTF-16 with BOM",
                e.utf8_error().valid_up_to()
            ),
        )
    })
}

fn utf16(bytes: &[u8], convert: fn([u8; 2]) -> u16) -> Result<String, Failure> {
    if !bytes.len().is_multiple_of(2) {
        return Err(Failure::new(
            "decode",
            "UTF-16 text has an odd number of bytes",
        ));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| convert([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| Failure::new("decode", "invalid UTF-16 text"))
}

/// 按行切分（保留空行），统一去掉 `\r`。
pub fn lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

pub fn parse(text: &str, ctx: &Ctx<'_>) -> ParseResult {
    let mut sink = BlockSink::default();
    let all = lines(text);
    let mut part = 1;
    let mut part_start = 0;
    let mut i = 0;
    while i < all.len() {
        ctx.check_time()?;
        if all[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < all.len() && !all[i].trim().is_empty() {
            i += 1;
        }
        if start - part_start >= LINES_PER_PART {
            part += 1;
            part_start = start;
        }
        let path = vec![format!("Part {part}")];
        let range = format!("{}-{}", start + 1, i);
        sink.push(
            BlockType::Paragraph,
            &path,
            locator("lines", range),
            all[start..i].join("\n"),
        );
    }
    if sink.blocks.len() > ctx.limits.max_records {
        return Err(Failure::limit(
            "text has more paragraphs than wiki.limits.maxRecords",
        ));
    }
    Ok(Parsed {
        blocks: sink.blocks,
        ..Default::default()
    })
}
