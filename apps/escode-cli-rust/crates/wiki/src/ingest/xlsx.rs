//! XLSX 解析（spec §7.5）：工作簿/工作表名称与顺序、隐藏工作表/行/列、命名区域、表格区域、批注、
//! 数据区域识别（空行分隔）、表头、重复表头跳过、合并单元格，按区域与 50 行分块，定位到单元格范围。
use super::ooxml::{Package, attr, core_properties};
use super::xlsx_cells::{Sheet, cell_ref, parse_sheet, shared_strings};
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};
use quick_xml::Reader;
use quick_xml::events::Event;

const ROWS_PER_BLOCK: u32 = 50;

struct SheetRef {
    name: String,
    hidden: bool,
    target: String,
}

/// 工作表清单与命名区域（名称 = 引用）。
type Workbook = (Vec<SheetRef>, Vec<(String, String)>);

fn workbook(package: &mut Package) -> Result<Workbook, Failure> {
    let rels = package.relationships("xl/_rels/workbook.xml.rels")?;
    let text = package
        .text("xl/workbook.xml")?
        .ok_or_else(|| Failure::parse("xl/workbook.xml is missing"))?;
    let mut reader = Reader::from_str(&text);
    let (mut sheets, mut names) = (vec![], vec![]);
    let mut defined: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == "sheet" => {
                let name = attr(&e, "name").unwrap_or_default();
                let hidden = attr(&e, "state").is_some_and(|s| s != "visible");
                let target = super::ooxml::rel_id(&e)
                    .and_then(|id| rels.get(&id).cloned())
                    .unwrap_or_default();
                let target = target
                    .trim_start_matches('/')
                    .trim_start_matches("xl/")
                    .to_owned();
                sheets.push(SheetRef {
                    name,
                    hidden,
                    target: format!("xl/{target}"),
                });
            }
            Ok(Event::Start(e)) if e.local_name().as_ref() == "definedName" => {
                defined = attr(&e, "name")
            }
            Ok(Event::Text(t)) => {
                if let Some(name) = defined.take() {
                    names.push((name, t.to_string()))
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Ok((sheets, names))
}

/// 同一区域内与表头完全相同的行视为重复表头（多段粘贴常见），跳过并告警。
fn regions(sheet: &Sheet) -> Vec<(u32, u32, u32, u32)> {
    let mut rows: Vec<u32> = sheet.cells.keys().map(|(r, _)| *r).collect();
    rows.dedup();
    let mut out = vec![];
    let mut start: Option<u32> = None;
    let mut prev = 0;
    for &row in &rows {
        match start {
            Some(_) if row == prev + 1 => {}
            Some(s) => {
                out.push((s, prev));
                start = Some(row);
            }
            None => start = Some(row),
        }
        prev = row;
    }
    if let Some(s) = start {
        out.push((s, prev));
    }
    out.into_iter()
        .map(|(top, bottom)| {
            let cols = sheet
                .cells
                .range((top, 0)..=(bottom, u32::MAX))
                .map(|((_, c), _)| *c);
            let (min, max) = cols.fold((u32::MAX, 0), |(lo, hi), c| (lo.min(c), hi.max(c)));
            (top, bottom, min, max)
        })
        .collect()
}

fn row_values(sheet: &Sheet, row: u32, min: u32, max: u32) -> Vec<String> {
    (min..=max)
        .map(|c| {
            sheet
                .cells
                .get(&(row, c))
                .map(super::xlsx_cells::Cell::display)
                .unwrap_or_default()
        })
        .collect()
}

fn esc(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
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
    let strings = package
        .text("xl/sharedStrings.xml")?
        .map(|x| shared_strings(&x))
        .unwrap_or_default();
    let (sheets, names) = workbook(&mut package)?;
    parsed.metadata.insert(
        "sheets".into(),
        sheets
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .into(),
    );
    if !names.is_empty() {
        parsed.metadata.insert(
            "definedNames".into(),
            names
                .iter()
                .map(|(n, r)| format!("{n} = {r}"))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    let tables: Vec<String> = package
        .names()
        .into_iter()
        .filter(|n| n.starts_with("xl/tables/") && n.ends_with(".xml"))
        .collect();
    let mut table_regions = vec![];
    for name in tables {
        if let Some(xml) = package.text(&name)? {
            let mut reader = Reader::from_str(&xml);
            if let Ok(Event::Start(e) | Event::Empty(e)) = reader.read_event().and_then(|ev| {
                if matches!(ev, Event::Decl(_)) {
                    reader.read_event()
                } else {
                    Ok(ev)
                }
            }) {
                table_regions.push(format!(
                    "{} = {}",
                    attr(&e, "displayName").unwrap_or_default(),
                    attr(&e, "ref").unwrap_or_default()
                ));
            }
        }
    }
    if !table_regions.is_empty() {
        parsed
            .metadata
            .insert("tables".into(), table_regions.into());
    }
    let mut budget = ctx.limits.max_sheet_cells;
    let mut sink = BlockSink::default();
    for sheet_ref in &sheets {
        ctx.check_time()?;
        let Some(xml) = package.text(&sheet_ref.target)? else {
            parsed.warnings.push(format!(
                "sheet '{}' has no worksheet part (chart sheet or missing)",
                sheet_ref.name
            ));
            continue;
        };
        let sheet = parse_sheet(&xml, &strings, &mut budget, ctx)?;
        if sheet.truncated {
            parsed.degraded = true;
            parsed.warnings.push(format!(
                "cell limit {} reached; sheet '{}' and later cells were truncated",
                ctx.limits.max_sheet_cells, sheet_ref.name
            ));
        }
        let label = if sheet_ref.hidden {
            parsed
                .warnings
                .push(format!("sheet '{}' is hidden", sheet_ref.name));
            format!("{} (hidden)", sheet_ref.name)
        } else {
            sheet_ref.name.clone()
        };
        for (index, (top, bottom, min, max)) in regions(&sheet).into_iter().enumerate() {
            let header = row_values(&sheet, top, min, max);
            let path = vec![label.clone(), format!("Table {}", index + 1)];
            let mut notes = vec![];
            let merged: Vec<&String> = sheet
                .merged
                .iter()
                .filter(|m| {
                    m.split(':')
                        .next()
                        .and_then(super::xlsx_cells::parse_ref)
                        .is_some_and(|(r, c)| {
                            (top..=bottom).contains(&r) && (min..=max).contains(&c)
                        })
                })
                .collect();
            if !merged.is_empty() {
                notes.push(format!(
                    "Merged cells: {}",
                    merged
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let hidden_rows: Vec<String> = sheet
                .hidden_rows
                .range(top..=bottom)
                .map(u32::to_string)
                .collect();
            if !hidden_rows.is_empty() {
                notes.push(format!("Hidden rows: {}", hidden_rows.join(", ")));
            }
            let hidden_cols: Vec<String> = sheet
                .hidden_cols
                .range(min..=max)
                .map(|c| super::xlsx_cells::column_name(*c))
                .collect();
            if !hidden_cols.is_empty() {
                notes.push(format!("Hidden columns: {}", hidden_cols.join(", ")));
            }
            let mut data_rows: Vec<u32> = vec![];
            for row in top + 1..=bottom {
                if row_values(&sheet, row, min, max) == header {
                    parsed.warnings.push(format!(
                        "sheet '{}' row {row} repeats the header and was skipped",
                        sheet_ref.name
                    ));
                } else {
                    data_rows.push(row);
                }
            }
            let chunks: Vec<&[u32]> = if data_rows.is_empty() {
                vec![&[]]
            } else {
                data_rows.chunks(ROWS_PER_BLOCK as usize).collect()
            };
            for (chunk_index, chunk) in chunks.into_iter().enumerate() {
                let first = chunk.first().copied().unwrap_or(top);
                let last = chunk.last().copied().unwrap_or(top);
                let range = format!(
                    "{}:{}",
                    cell_ref(if chunk_index == 0 { top } else { first }, min),
                    cell_ref(last, max)
                );
                let mut text = String::new();
                if chunk_index == 0 && !notes.is_empty() {
                    text.push_str(&notes.join("\n"));
                    text.push_str("\n\n");
                }
                text.push_str(&format!(
                    "| row | {} |\n|{}|",
                    header
                        .iter()
                        .map(|h| esc(h))
                        .collect::<Vec<_>>()
                        .join(" | "),
                    " --- |".repeat(header.len() + 1)
                ));
                for row in chunk {
                    text.push_str(&format!(
                        "\n| {row} | {} |",
                        row_values(&sheet, *row, min, max)
                            .iter()
                            .map(|v| esc(v))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
                let mut block_path = path.clone();
                if chunk_index > 0 {
                    block_path.push(format!("Part {}", chunk_index + 1));
                }
                sink.push(
                    BlockType::Table,
                    &block_path,
                    locator("cells", format!("Sheet: {} → {range}", sheet_ref.name)),
                    text,
                );
            }
        }
        comments(&mut package, sheet_ref, &label, &mut sink)?;
    }
    parsed.blocks = sink.blocks;
    Ok(parsed)
}

/// 工作表批注（经 sheet rels 找到 comments 部件）。
fn comments(
    package: &mut Package,
    sheet: &SheetRef,
    label: &str,
    sink: &mut BlockSink,
) -> Result<(), Failure> {
    let (dir, file) = sheet
        .target
        .rsplit_once('/')
        .unwrap_or(("xl/worksheets", &sheet.target));
    let rels = package.relationships(&format!("{dir}/_rels/{file}.rels"))?;
    for target in rels.values().filter(|t| t.contains("comments")) {
        let part = if let Some(rest) = target.strip_prefix("../") {
            format!("xl/{rest}")
        } else {
            format!("{dir}/{target}")
        };
        let Some(xml) = package.text(&part)? else {
            continue;
        };
        let mut reader = Reader::from_str(&xml);
        let (mut current, mut buffer, mut in_t) = (None::<String>, String::new(), false);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) if e.local_name().as_ref() == "comment" => {
                    current = attr(&e, "ref")
                }
                Ok(Event::Start(e)) if e.local_name().as_ref() == "t" => in_t = true,
                Ok(Event::End(e)) if e.local_name().as_ref() == "t" => in_t = false,
                Ok(Event::Text(t)) if in_t => buffer.push_str(&t),
                Ok(Event::End(e)) if e.local_name().as_ref() == "comment" => {
                    if let Some(cell) = current.take() {
                        sink.push(
                            BlockType::Comment,
                            &[label.to_owned(), "Comments".into()],
                            locator("cells", format!("Sheet: {} → {cell}", sheet.name)),
                            format!("{cell}: {}", buffer.trim()),
                        );
                    }
                    buffer.clear();
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    Ok(())
}
