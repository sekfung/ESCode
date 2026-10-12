//! reStructuredText 基础解析：下划线/上划线标题（层级按首次出现的装饰样式）、`::` 字面块、列表、段落。
//! 指令（`.. name::`）不解释，原文保留并告警（spec §7.2「明确无法解析的扩展语法」）。
use super::text::lines;
use super::{Ctx, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};

const ADORNMENT: &[u8] = b"=-~^\"'*+#:_`.";

fn underline(line: &str, title: &str) -> Option<u8> {
    let t = line.trim_end();
    let first = *t.as_bytes().first()?;
    (ADORNMENT.contains(&first)
        && t.bytes().all(|b| b == first)
        && t.chars().count() >= title.trim().chars().count().max(2))
    .then_some(first)
}

pub fn parse(text: &str, ctx: &Ctx<'_>) -> ParseResult {
    let all = lines(text);
    let mut parsed = Parsed::default();
    let mut styles: Vec<(u8, bool)> = vec![];
    let mut stack: Vec<(usize, String)> = vec![];
    let mut sink = BlockSink::default();
    let path =
        |stack: &Vec<(usize, String)>| stack.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
    let mut i = 0;
    while i < all.len() {
        ctx.check_time()?;
        let line = all[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        // 上划线 + 标题 + 下划线
        let over = underline(line, "").filter(|_| {
            all.get(i + 2)
                .is_some_and(|u| underline(u, all[i + 1]).is_some())
        });
        let under = if over.is_none() {
            all.get(i + 1).and_then(|u| underline(u, line))
        } else {
            None
        };
        if over.is_some() || (under.is_some() && underline(line, "").is_none()) {
            let (title, style, consumed) = match over {
                Some(c) => (all[i + 1].trim().to_owned(), (c, true), 3),
                None => (line.trim().to_owned(), (under.unwrap_or(b'='), false), 2),
            };
            let level = styles.iter().position(|s| *s == style).unwrap_or_else(|| {
                styles.push(style);
                styles.len() - 1
            });
            stack.retain(|(l, _)| *l < level);
            stack.push((level, title.clone()));
            if parsed.title.is_none() {
                parsed.title = Some(title.clone());
            }
            let block = sink.push(
                BlockType::Heading,
                &path(&stack),
                locator("lines", (i + 1).to_string()),
                title,
            );
            block.level = Some(level as u8 + 1);
            i += consumed;
            continue;
        }
        if let Some(name) = line
            .trim_start()
            .strip_prefix(".. ")
            .and_then(|r| r.split_once("::"))
            .map(|(n, _)| n.trim().to_owned())
        {
            parsed.warnings.push(format!(
                "line {}: directive '{name}' is kept as text and not interpreted",
                i + 1
            ));
        }
        let begin = i;
        while i < all.len() && !all[i].trim().is_empty() {
            i += 1;
        }
        let paragraph = all[begin..i].join("\n");
        let t = paragraph.trim_start();
        let kind = if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("#. ") {
            BlockType::ListItem
        } else {
            BlockType::Paragraph
        };
        sink.push(
            kind,
            &path(&stack),
            locator("lines", format!("{}-{}", begin + 1, i)),
            paragraph.clone(),
        );
        if paragraph.trim_end().ends_with("::") {
            // 字面块：跳过空行后的缩进段。
            let mut j = i;
            while j < all.len() && all[j].trim().is_empty() {
                j += 1;
            }
            let code_start = j;
            while j < all.len() && (all[j].trim().is_empty() || all[j].starts_with([' ', '\t'])) {
                j += 1;
            }
            if j > code_start {
                let body = all[code_start..j]
                    .iter()
                    .map(|l| l.trim_start())
                    .collect::<Vec<_>>()
                    .join("\n");
                sink.push(
                    BlockType::Code,
                    &path(&stack),
                    locator("lines", format!("{}-{}", code_start + 1, j)),
                    body.trim_end().to_owned(),
                );
                i = j;
            }
        }
    }
    parsed.blocks = sink.blocks;
    Ok(parsed)
}
