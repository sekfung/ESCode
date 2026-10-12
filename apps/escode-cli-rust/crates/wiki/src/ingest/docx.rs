//! DOCX 解析（spec §7.4）：标题层级（按 styles.xml 的样式名 `heading N`/`Title` 与 outlineLvl，兼容本地化样式 ID）、
//! 段落顺序、列表层级、表格行列、超链接、脚注/尾注、页眉页脚、批注与核心属性。图片与嵌入对象报告为未解析。
use super::ooxml::{Package, attr, core_properties};
use super::xmlutil::entity;
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;

/// styleId → 标题级别（1..9）；0 表示 Title。
fn heading_styles(package: &mut Package) -> Result<HashMap<String, u8>, Failure> {
    let mut map = HashMap::new();
    let Some(text) = package.text("word/styles.xml")? else {
        return Ok(map);
    };
    let mut reader = Reader::from_str(&text);
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "style" => {
                current = attr(&e, "styleId")
            }
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                let Some(id) = current.clone() else { continue };
                match e.local_name().as_ref() {
                    "name" => {
                        let name = attr(&e, "val").unwrap_or_default().to_lowercase();
                        if name == "title" {
                            map.insert(id, 0);
                        } else if let Some(n) = name
                            .strip_prefix("heading ")
                            .and_then(|n| n.trim().parse::<u8>().ok())
                        {
                            map.insert(id, n.clamp(1, 9));
                        }
                    }
                    "outlineLvl" => {
                        if let Some(n) = attr(&e, "val").and_then(|v| v.parse::<u8>().ok()) {
                            map.entry(id).or_insert(n.saturating_add(1).clamp(1, 9));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "style" => current = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Ok(map)
}

#[derive(Default)]
struct Para {
    text: String,
    style: Option<String>,
    outline: Option<u8>,
    list_level: Option<u8>,
    /// 超链接目标；正文里用 \u{1}…\u{2} 标出链接文字的范围。
    links: Vec<String>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    cell: Option<String>,
}

fn table_markdown(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let line = |row: &Vec<String>| {
        let cells: Vec<String> = (0..width)
            .map(|i| {
                row.get(i)
                    .map_or(String::new(), |c| c.replace('|', "\\|").replace('\n', " "))
            })
            .collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![];
    for (i, row) in rows.iter().enumerate() {
        out.push(line(row));
        if i == 0 {
            out.push(format!("|{}", " --- |".repeat(width)));
        }
    }
    out.join("\n")
}

pub fn parse(bytes: &[u8], ctx: &Ctx<'_>) -> ParseResult {
    let mut package = Package::open(bytes, ctx)?;
    let mut parsed = Parsed {
        warnings: std::mem::take(&mut package.warnings),
        ..Default::default()
    };
    parsed.metadata = core_properties(&mut package)?;
    parsed.title = parsed
        .metadata
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let styles = heading_styles(&mut package)?;
    let links = package.relationships("word/_rels/document.xml.rels")?;
    let document = package
        .text("word/document.xml")?
        .ok_or_else(|| Failure::parse("word/document.xml is missing"))?;
    let mut sink = BlockSink::default();
    let mut stack: Vec<(u8, String)> = vec![];
    let mut reader = Reader::from_str(&document);
    let mut para: Option<Para> = None;
    let mut tables: Vec<Table> = vec![];
    let mut in_text = false;
    let mut paragraph_index = 0usize;
    let mut table_index = 0usize;
    let mut images = 0usize;
    let mut events = 0usize;
    let path = |stack: &Vec<(u8, String)>| stack.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
    loop {
        events += 1;
        if events.is_multiple_of(2000) {
            ctx.check_time()?;
        }
        let event = reader
            .read_event()
            .map_err(|e| Failure::parse(format!("document.xml: {e}")))?;
        match event {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                "p" => para = Some(Para::default()),
                "pStyle" => {
                    if let Some(p) = para.as_mut() {
                        p.style = attr(&e, "val")
                    }
                }
                "outlineLvl" => {
                    if let Some(p) = para.as_mut() {
                        p.outline = attr(&e, "val").and_then(|v| v.parse().ok())
                    }
                }
                "ilvl" => {
                    if let Some(p) = para.as_mut() {
                        p.list_level = attr(&e, "val").and_then(|v| v.parse().ok())
                    }
                }
                "numPr" => {
                    if let Some(p) = para.as_mut() {
                        p.list_level.get_or_insert(0);
                    }
                }
                "t" => in_text = true,
                "tab" => {
                    if let Some(p) = para.as_mut() {
                        p.text.push('\t')
                    }
                }
                "br" | "cr" => {
                    if let Some(p) = para.as_mut() {
                        p.text.push('\n')
                    }
                }
                "hyperlink" => {
                    if let Some(p) = para.as_mut() {
                        // 内部书签（w:anchor）没有外部目标，只保留文字。
                        p.links.push(
                            super::ooxml::rel_id(&e)
                                .and_then(|id| links.get(&id).cloned())
                                .unwrap_or_default(),
                        );
                        p.text.push('\u{1}');
                    }
                }
                "footnoteReference" | "endnoteReference" => {
                    if let (Some(p), Some(id)) = (para.as_mut(), attr(&e, "id")) {
                        p.text.push_str(&format!("[^{id}]"));
                    }
                }
                "drawing" | "pict" | "object" => images += 1,
                "tbl" => tables.push(Table::default()),
                "tr" => {
                    if let Some(t) = tables.last_mut() {
                        t.rows.push(vec![])
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut() {
                        t.cell = Some(String::new())
                    }
                }
                _ => {}
            },
            Event::Text(t) if in_text => {
                if let Some(p) = para.as_mut() {
                    p.text.push_str(&t)
                }
            }
            Event::GeneralRef(r) if in_text => {
                if let Some(p) = para.as_mut() {
                    p.text.push_str(&entity(&r))
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "hyperlink" => {
                    if let Some(p) = para.as_mut() {
                        p.text.push('\u{2}')
                    }
                }
                "p" => {
                    let Some(p) = para.take() else { continue };
                    let text = render_links(&p.text, &p.links);
                    if let Some(table) = tables.last_mut() {
                        if let Some(cell) = table.cell.as_mut() {
                            if !cell.is_empty() && !text.is_empty() {
                                cell.push('\n');
                            }
                            cell.push_str(text.trim());
                        }
                        continue;
                    }
                    if text.trim().is_empty() {
                        continue;
                    }
                    paragraph_index += 1;
                    let level = p
                        .style
                        .as_ref()
                        .and_then(|s| styles.get(s).copied())
                        .or(p.outline.map(|o| o + 1));
                    let loc = locator("paragraph", format!("paragraph {paragraph_index}"));
                    match level {
                        Some(0) => {
                            parsed.title.get_or_insert_with(|| text.trim().to_owned());
                        }
                        Some(level) => {
                            stack.retain(|(l, _)| *l < level);
                            stack.push((level, text.trim().to_owned()));
                            sink.push(BlockType::Heading, &path(&stack), loc, text.trim())
                                .level = Some(level);
                        }
                        None if p.list_level.is_some() => {
                            let indent = "  ".repeat(usize::from(p.list_level.unwrap_or(0)));
                            sink.push(
                                BlockType::ListItem,
                                &path(&stack),
                                loc,
                                format!("{indent}{}", text.trim()),
                            );
                        }
                        None => {
                            sink.push(BlockType::Paragraph, &path(&stack), loc, text.trim());
                        }
                    }
                }
                "tc" => {
                    if let Some(t) = tables.last_mut()
                        && let (Some(cell), Some(row)) = (t.cell.take(), t.rows.last_mut())
                    {
                        row.push(cell);
                    }
                }
                "tbl" => {
                    let Some(table) = tables.pop() else { continue };
                    let rows: Vec<Vec<String>> =
                        table.rows.into_iter().filter(|r| !r.is_empty()).collect();
                    if let Some(outer) = tables.last_mut() {
                        // 嵌套表格压平进外层单元格。
                        if let Some(cell) = outer.cell.as_mut() {
                            cell.push_str(&table_markdown(&rows).replace('\n', " "));
                        }
                        continue;
                    }
                    if rows.is_empty() {
                        continue;
                    }
                    table_index += 1;
                    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let loc = locator(
                        "table",
                        format!("table {table_index} ({} rows × {cols} columns)", rows.len()),
                    );
                    sink.push(BlockType::Table, &path(&stack), loc, table_markdown(&rows));
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    if images > 0 {
        parsed.warnings.push(format!(
            "{images} images or embedded objects were not parsed"
        ));
    }
    for (part, label, kind) in [
        ("word/footnotes.xml", "Footnotes", BlockType::Footnote),
        ("word/endnotes.xml", "Endnotes", BlockType::Footnote),
        ("word/comments.xml", "Comments", BlockType::Comment),
    ] {
        notes(&mut package, part, label, kind, &mut sink)?;
    }
    for name in package.names().into_iter().filter(|n| {
        (n.starts_with("word/header") || n.starts_with("word/footer")) && n.ends_with(".xml")
    }) {
        notes(
            &mut package,
            &name,
            "Headers and footers",
            BlockType::HeaderFooter,
            &mut sink,
        )?;
    }
    parsed.blocks = sink.blocks;
    Ok(parsed)
}

/// 把 \u{1}文字\u{2} 渲染成 Markdown 链接 `[文字](目标)`；没有外部目标时只留文字。
fn render_links(text: &str, links: &[String]) -> String {
    let mut out = String::new();
    let mut targets = links.iter();
    let mut label: Option<String> = None;
    for c in text.chars() {
        match c {
            '\u{1}' => label = Some(String::new()),
            '\u{2}' => {
                let text = label.take().unwrap_or_default();
                match targets.next().filter(|t| !t.is_empty()) {
                    Some(target) => out.push_str(&format!("[{}]({target})", text.trim())),
                    None => out.push_str(&text),
                }
            }
            c => match label.as_mut() {
                Some(label) => label.push(c),
                None => out.push(c),
            },
        }
    }
    if let Some(rest) = label {
        out.push_str(&rest);
    }
    out
}

/// 脚注/尾注/批注/页眉页脚：逐条（按 w:id）或整段文本作为块。
fn notes(
    package: &mut Package,
    part: &str,
    label: &str,
    kind: BlockType,
    sink: &mut BlockSink,
) -> Result<(), Failure> {
    let Some(text) = package.text(part)? else {
        return Ok(());
    };
    let mut reader = Reader::from_str(&text);
    let (mut current, mut buffer, mut in_text) = (None::<String>, String::new(), false);
    let mut flush = |id: Option<String>, buffer: &mut String| {
        let body = buffer.trim().to_owned();
        buffer.clear();
        let id = id.unwrap_or_default();
        // w:id -1 / 0 是分隔符脚注。
        if !body.is_empty() && id != "-1" && id != "0" {
            let value = if id.is_empty() {
                part.to_owned()
            } else {
                format!("{part} #{id}")
            };
            sink.push(kind, &[label.to_owned()], locator("paragraph", value), body);
        }
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e))
                if matches!(e.local_name().as_ref(), "footnote" | "endnote" | "comment") =>
            {
                current = attr(&e, "id")
            }
            Ok(Event::Start(e)) if e.local_name().as_ref() == "t" => in_text = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == "t" => in_text = false,
            Ok(Event::End(e)) if e.local_name().as_ref() == "p" => buffer.push('\n'),
            Ok(Event::Text(t)) if in_text => buffer.push_str(&t),
            Ok(Event::GeneralRef(r)) if in_text => buffer.push_str(&entity(&r)),
            Ok(Event::End(e))
                if matches!(e.local_name().as_ref(), "footnote" | "endnote" | "comment") =>
            {
                flush(current.take(), &mut buffer)
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    flush(None, &mut buffer);
    Ok(())
}
