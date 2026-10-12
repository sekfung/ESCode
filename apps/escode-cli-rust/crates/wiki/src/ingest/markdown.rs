//! Markdown 解析（spec §7.7）：frontmatter、ATX/Setext 标题、列表、引用、围栏代码（语言）、表格、
//! 链接与图片引用（保留原文与相对路径）。按行实现，定位精确到行号。
use super::text::lines;
use super::{Ctx, ParseResult, Parsed};
use escode_cli_domain::wiki::frontmatter::parse_loose;
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};
use serde_json::Value;

fn atx(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some((
        hashes as u8,
        rest.trim().trim_end_matches('#').trim().to_owned(),
    ))
}

fn fence(line: &str) -> Option<(&str, String)> {
    let trimmed = line.trim_start();
    for marker in ["```", "~~~"] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((
                marker,
                rest.trim().trim_start_matches(['`', '~']).to_owned(),
            ));
        }
    }
    None
}

fn list_item(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || t.split_once(". ").is_some_and(|(n, _)| {
            !n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit())
        })
}

pub fn parse(text: &str, ctx: &Ctx<'_>) -> ParseResult {
    let all = lines(text);
    let mut parsed = Parsed::default();
    let mut start = 0;
    if all.first().is_some_and(|l| l.trim() == "---")
        && let Some((meta, body)) = parse_loose(text)
        && let Some(end) = all.iter().skip(1).position(|l| l.trim() == "---")
    {
        if let Value::Object(map) = &meta {
            parsed.title = map.get("title").and_then(Value::as_str).map(str::to_owned);
            parsed.metadata.insert("frontmatter".into(), meta.clone());
            // 已是 Wiki 条目格式时保留原始正文（按原 ID 导入，不经过章节抽取改写格式）。
            if map.contains_key("schema_version") && map.contains_key("id") {
                parsed
                    .metadata
                    .insert("frontmatterBody".into(), body.into());
            }
        }
        start = end + 2;
    }
    let mut sink = BlockSink::default();
    let mut stack: Vec<(u8, String)> = vec![];
    let path = |stack: &Vec<(u8, String)>| stack.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
    let mut i = start;
    while i < all.len() {
        ctx.check_time()?;
        let line = all[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let setext = all.get(i + 1).and_then(|next| {
            let t = next.trim();
            (!t.is_empty()
                && !list_item(line)
                && (t.bytes().all(|b| b == b'=') || (t.len() >= 3 && t.bytes().all(|b| b == b'-'))))
            .then(|| if t.starts_with('=') { 1 } else { 2 })
        });
        let heading = atx(line).or_else(|| setext.map(|level| (level, line.trim().to_owned())));
        if let Some((level, title)) = heading {
            stack.retain(|(l, _)| *l < level);
            stack.push((level, title.clone()));
            if parsed.title.is_none() && level == 1 {
                parsed.title = Some(title.clone());
            }
            let consumed = if setext.is_some() && atx(line).is_none() {
                2
            } else {
                1
            };
            let block = sink.push(
                BlockType::Heading,
                &path(&stack),
                locator("lines", (i + 1).to_string()),
                title,
            );
            block.level = Some(level);
            i += consumed;
            continue;
        }
        if let Some((marker, lang)) = fence(line) {
            let begin = i;
            i += 1;
            while i < all.len() && !all[i].trim_start().starts_with(marker) {
                i += 1;
            }
            let body = all[begin + 1..i.min(all.len())].join("\n");
            let end = i.min(all.len() - 1);
            let block = sink.push(
                BlockType::Code,
                &path(&stack),
                locator("lines", format!("{}-{}", begin + 1, end + 1)),
                body,
            );
            block.lang = (!lang.is_empty()).then_some(lang);
            i += 1;
            continue;
        }
        let begin = i;
        let kind = if line.trim_start().starts_with('|') {
            BlockType::Table
        } else if line.trim_start().starts_with('>') {
            BlockType::Quote
        } else if list_item(line) {
            BlockType::ListItem
        } else {
            BlockType::Paragraph
        };
        i += 1;
        while i < all.len() {
            let next = all[i];
            let continues = match kind {
                BlockType::Table => next.trim_start().starts_with('|'),
                BlockType::Quote => next.trim_start().starts_with('>'),
                BlockType::ListItem => {
                    !next.trim().is_empty() && !list_item(next) && next.starts_with([' ', '\t'])
                }
                _ => {
                    !next.trim().is_empty()
                        && atx(next).is_none()
                        && fence(next).is_none()
                        && !list_item(next)
                        && !next.trim_start().starts_with(['|', '>'])
                }
            };
            if !continues {
                break;
            }
            i += 1;
        }
        let mut body = all[begin..i].join("\n");
        if kind == BlockType::Quote {
            body = body
                .lines()
                .map(|l| l.trim_start().trim_start_matches('>').trim_start())
                .collect::<Vec<_>>()
                .join("\n");
        }
        sink.push(
            kind,
            &path(&stack),
            locator("lines", format!("{}-{}", begin + 1, i)),
            body,
        );
    }
    if sink.blocks.len() > ctx.limits.max_records {
        return Err(super::Failure::limit(
            "markdown has more blocks than wiki.limits.maxRecords",
        ));
    }
    parsed.blocks = sink.blocks;
    Ok(parsed)
}
