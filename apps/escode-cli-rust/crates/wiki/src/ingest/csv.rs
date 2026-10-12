//! CSV/TSV 解析（spec §7.7）：RFC 4180 引号与转义、表头修复（空表头 → column_N，重复 → name_2）、
//! 无表头识别（首行全为数字）、按记录分块（每块 50 条），定位为文件行号与记录号。
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};

const RECORDS_PER_BLOCK: usize = 50;

/// 返回 (记录, 每条记录的起始行号)。
fn records(
    text: &str,
    delimiter: u8,
    ctx: &Ctx<'_>,
) -> Result<(Vec<Vec<String>>, Vec<usize>), Failure> {
    let mut rows = vec![];
    let mut starts = vec![];
    let mut row: Vec<String> = vec![];
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1usize;
    let mut row_start = 1usize;
    let mut chars = text.chars().peekable();
    let delimiter = delimiter as char;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                    starts.push(row_start);
                } else {
                    row.clear();
                }
                line += 1;
                row_start = line;
                if rows.len() > ctx.limits.max_records {
                    return Err(Failure::limit(format!(
                        "more than {} records (wiki.limits.maxRecords)",
                        ctx.limits.max_records
                    )));
                }
                if rows.len().is_multiple_of(1000) {
                    ctx.check_time()?;
                }
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err(Failure::parse(format!(
            "unterminated quoted field starting in the record at line {row_start}"
        )));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
        starts.push(row_start);
    }
    Ok((rows, starts))
}

fn repair_header(raw: &[String], warnings: &mut Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    raw.iter()
        .enumerate()
        .map(|(i, name)| {
            let base = if name.trim().is_empty() {
                warnings.push(format!(
                    "column {} has an empty header; named column_{}",
                    i + 1,
                    i + 1
                ));
                format!("column_{}", i + 1)
            } else {
                name.trim().to_owned()
            };
            let count = seen.entry(base.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                warnings.push(format!(
                    "duplicate header '{base}' renamed to {base}_{count}"
                ));
                format!("{base}_{count}")
            } else {
                base
            }
        })
        .collect()
}

fn escape_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

pub fn parse(text: &str, delimiter: u8, ctx: &Ctx<'_>) -> ParseResult {
    let (rows, starts) = records(text, delimiter, ctx)?;
    let mut parsed = Parsed::default();
    let Some(first) = rows.first() else {
        return Err(Failure::parse("file contains no records"));
    };
    let numeric = |s: &String| s.trim().parse::<f64>().is_ok();
    let headerless = first.iter().all(numeric);
    let (header, data_start) = if headerless {
        parsed
            .warnings
            .push("first row looks like data; synthetic column names used".into());
        (
            (1..=first.len()).map(|i| format!("column_{i}")).collect(),
            0,
        )
    } else {
        (repair_header(first, &mut parsed.warnings), 1)
    };
    parsed
        .metadata
        .insert("columns".into(), header.clone().into());
    let width = header.len();
    let mut ragged = 0;
    let mut sink = BlockSink::default();
    let data = &rows[data_start..];
    for (chunk_index, chunk) in data.chunks(RECORDS_PER_BLOCK).enumerate() {
        ctx.check_time()?;
        let first_record = chunk_index * RECORDS_PER_BLOCK + 1;
        let last_record = first_record + chunk.len() - 1;
        let first_line = starts[data_start + first_record - 1];
        let last_line = starts[data_start + last_record - 1];
        let mut table = format!(
            "| record | {} |\n|{}|",
            header
                .iter()
                .map(|h| escape_cell(h))
                .collect::<Vec<_>>()
                .join(" | "),
            " --- |".repeat(width + 1)
        );
        for (offset, row) in chunk.iter().enumerate() {
            if row.len() != width {
                ragged += 1;
            }
            let cells: Vec<String> = (0..width)
                .map(|i| row.get(i).map_or(String::new(), |c| escape_cell(c)))
                .collect();
            table.push_str(&format!(
                "\n| {} | {} |",
                first_record + offset,
                cells.join(" | ")
            ));
        }
        let path = vec![format!("Records {first_record}-{last_record}")];
        sink.push(
            BlockType::Table,
            &path,
            locator(
                "record",
                format!("records {first_record}-{last_record} (lines {first_line}-{last_line})"),
            ),
            table,
        );
    }
    if ragged > 0 {
        parsed.warnings.push(format!(
            "{ragged} records have a different number of fields than the header"
        ));
    }
    parsed.metadata.insert("records".into(), data.len().into());
    parsed.blocks = sink.blocks;
    Ok(parsed)
}
