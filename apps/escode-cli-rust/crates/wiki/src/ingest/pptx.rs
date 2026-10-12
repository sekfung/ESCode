//! PPTX 解析（spec §7.2）：幻灯片顺序（presentation.xml 的 sldIdLst）、标题占位符、文本框、表格与备注。
//! 图片、图表报告为未解析。
use super::ooxml::{Package, attr, core_properties};
use super::xmlutil::entity;
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};
use quick_xml::Reader;
use quick_xml::events::Event;

fn resolve(base_dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = base_dir.split('/').filter(|p| !p.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

struct SlideText {
    title: Option<String>,
    paragraphs: Vec<String>,
    tables: Vec<Vec<Vec<String>>>,
    pictures: usize,
}

fn slide_text(xml: &str) -> SlideText {
    let mut out = SlideText {
        title: None,
        paragraphs: vec![],
        tables: vec![],
        pictures: 0,
    };
    let mut reader = Reader::from_str(xml);
    let (mut in_title_shape, mut in_t, mut para) = (false, false, String::new());
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match e.local_name().as_ref() {
                "sp" => in_title_shape = false,
                "ph" => {
                    in_title_shape =
                        matches!(attr(&e, "type").as_deref(), Some("title" | "ctrTitle"))
                }
                "t" => in_t = true,
                "pic" | "graphicFrame" if table.is_none() => {
                    out.pictures += usize::from(e.local_name().as_ref() == "pic")
                }
                "tbl" => table = Some(vec![]),
                "tr" => {
                    if let Some(t) = table.as_mut() {
                        t.push(vec![])
                    }
                }
                "tc" => cell = Some(String::new()),
                _ => {}
            },
            Ok(Event::Text(t)) if in_t => para.push_str(&t),
            Ok(Event::GeneralRef(r)) if in_t => para.push_str(&entity(&r)),
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "t" => in_t = false,
                "p" => {
                    let text = std::mem::take(&mut para).trim().to_owned();
                    if text.is_empty() {
                        continue;
                    }
                    if let Some(c) = cell.as_mut() {
                        if !c.is_empty() {
                            c.push(' ');
                        }
                        c.push_str(&text);
                    } else if in_title_shape && out.title.is_none() {
                        out.title = Some(text);
                    } else {
                        out.paragraphs.push(text);
                    }
                }
                "tc" => {
                    if let (Some(t), Some(c)) = (table.as_mut(), cell.take())
                        && let Some(row) = t.last_mut()
                    {
                        row.push(c)
                    }
                }
                "tbl" => {
                    if let Some(t) = table.take() {
                        out.tables.push(t)
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
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
    let rels = package.relationships("ppt/_rels/presentation.xml.rels")?;
    let presentation = package
        .text("ppt/presentation.xml")?
        .ok_or_else(|| Failure::parse("ppt/presentation.xml is missing"))?;
    let mut order = vec![];
    let mut reader = Reader::from_str(&presentation);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == "sldId" => {
                if let Some(target) = super::ooxml::rel_id(&e).and_then(|id| rels.get(&id).cloned())
                {
                    order.push(resolve("ppt", &target));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let mut sink = BlockSink::default();
    let mut pictures = 0;
    for (index, part) in order.iter().enumerate() {
        ctx.check_time()?;
        let number = index + 1;
        let Some(xml) = package.text(part)? else {
            continue;
        };
        let slide = slide_text(&xml);
        pictures += slide.pictures;
        let heading = match &slide.title {
            Some(title) => format!("Slide {number}: {title}"),
            None => format!("Slide {number}"),
        };
        let path = vec![heading.clone()];
        let loc = || locator("slide", format!("slide {number}"));
        sink.push(BlockType::Slide, &path, loc(), heading.clone());
        for paragraph in &slide.paragraphs {
            sink.push(BlockType::Paragraph, &path, loc(), paragraph.clone());
        }
        for table in &slide.tables {
            let text = table
                .iter()
                .map(|r| format!("| {} |", r.join(" | ")))
                .collect::<Vec<_>>()
                .join("\n");
            sink.push(BlockType::Table, &path, loc(), text);
        }
        let (dir, file) = part.rsplit_once('/').unwrap_or(("ppt/slides", part));
        let slide_rels = package.relationships(&format!("{dir}/_rels/{file}.rels"))?;
        for target in slide_rels.values().filter(|t| t.contains("notesSlide")) {
            if let Some(notes) = package.text(&resolve(dir, target))? {
                let text = slide_text(&notes).paragraphs.join("\n");
                if !text.trim().is_empty() {
                    sink.push(
                        BlockType::Note,
                        &path,
                        locator("slide", format!("slide {number} notes")),
                        format!("Speaker notes: {text}"),
                    );
                }
            }
        }
    }
    if pictures > 0 {
        parsed
            .warnings
            .push(format!("{pictures} pictures/charts were not parsed"));
    }
    parsed.metadata.insert("slides".into(), order.len().into());
    parsed.blocks = sink.blocks;
    Ok(parsed)
}
