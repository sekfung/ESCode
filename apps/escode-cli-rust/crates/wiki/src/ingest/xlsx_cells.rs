//! XLSX 单元格层：共享字符串、单元格坐标、原始值 / 公式文本 / 缓存结果的区分（spec §7.5）。
//! 公式不当作结果，缓存值明确标注为 cached；共享公式标注来源（从主单元格推导，不重新计算）。
use super::xmlutil::entity;
use super::{Ctx, Failure};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    /// Excel 类型属性 t（s / str / inlineStr / b / e / n）。
    pub kind: String,
    /// 原始存储值（共享字符串已解引用）。
    pub raw: Option<String>,
    pub formula: Option<String>,
    /// 共享公式从主单元格推导时为 true（导入工具推导，不是文件原文）。
    pub formula_derived: bool,
}

impl Cell {
    /// 单元格展示：普通值原样；公式写成 `=公式 → 缓存值 (cached)`；无缓存值时明确说明。
    pub fn display(&self) -> String {
        let value = match (self.kind.as_str(), self.raw.as_deref()) {
            ("b", Some("1")) => "TRUE".to_owned(),
            ("b", Some(_)) => "FALSE".to_owned(),
            ("e", Some(v)) => format!("#ERROR {v}"),
            (_, Some(v)) => v.to_owned(),
            (_, None) => String::new(),
        };
        match &self.formula {
            Some(f) => {
                let derived = if self.formula_derived {
                    " (shared formula, derived)"
                } else {
                    ""
                };
                if value.is_empty() {
                    format!("={f}{derived} → (no cached value)")
                } else {
                    format!("={f}{derived} → {value} (cached)")
                }
            }
            None => value,
        }
    }
}

#[derive(Debug, Default)]
pub struct Sheet {
    /// (行, 列) 从 1 开始。
    pub cells: BTreeMap<(u32, u32), Cell>,
    pub merged: Vec<String>,
    pub hidden_rows: BTreeSet<u32>,
    pub hidden_cols: BTreeSet<u32>,
    pub truncated: bool,
}

pub fn column_name(mut col: u32) -> String {
    let mut out = vec![];
    while col > 0 {
        let rem = (col - 1) % 26;
        out.push((b'A' + rem as u8) as char);
        col = (col - 1) / 26;
    }
    out.iter().rev().collect()
}

pub fn parse_ref(reference: &str) -> Option<(u32, u32)> {
    let letters: String = reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    let digits = &reference[letters.len()..];
    let col = letters.bytes().try_fold(0u32, |acc, b| {
        acc.checked_mul(26)?
            .checked_add(u32::from(b.to_ascii_uppercase() - b'A' + 1))
    })?;
    let row = digits.parse().ok()?;
    (col > 0 && row > 0).then_some((row, col))
}

pub fn cell_ref(row: u32, col: u32) -> String {
    format!("{}{row}", column_name(col))
}

pub fn shared_strings(xml: &str) -> Vec<String> {
    let mut out = vec![];
    let mut reader = Reader::from_str(xml);
    let (mut current, mut in_si, mut in_t, mut in_phonetic) = (String::new(), false, false, false);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                "si" => {
                    in_si = true;
                    current.clear();
                }
                "t" => in_t = in_si,
                "rPh" => in_phonetic = true,
                _ => {}
            },
            Ok(Event::Empty(e)) if e.local_name().as_ref() == "si" => out.push(String::new()),
            Ok(Event::Text(t)) if in_t && !in_phonetic => current.push_str(&t),
            Ok(Event::GeneralRef(r)) if in_t && !in_phonetic => current.push_str(&entity(&r)),
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "si" => {
                    in_si = false;
                    out.push(std::mem::take(&mut current));
                }
                "t" => in_t = false,
                "rPh" => in_phonetic = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// 解析一个工作表；`budget` 为剩余可读单元格数（跨工作表共享），用尽时截断并标记。
pub fn parse_sheet(
    xml: &str,
    strings: &[String],
    budget: &mut usize,
    ctx: &Ctx<'_>,
) -> Result<Sheet, Failure> {
    let mut sheet = Sheet::default();
    let mut reader = Reader::from_str(xml);
    let mut shared: HashMap<String, String> = HashMap::new();
    let mut cell: Option<((u32, u32), Cell)> = None;
    let mut field: Option<&'static str> = None;
    let mut buffer = String::new();
    let mut pending_shared: Option<String> = None;
    let mut events = 0usize;
    loop {
        events += 1;
        if events.is_multiple_of(5000) {
            ctx.check_time()?;
        }
        let event = reader
            .read_event()
            .map_err(|e| Failure::parse(format!("worksheet XML: {e}")))?;
        // 自闭合 `<f t="shared" si="0"/>`（从属单元格）没有 End 事件。
        let self_closing = matches!(event, Event::Empty(_));
        match event {
            Event::Start(e) | Event::Empty(e) => {
                match e.local_name().as_ref() {
                    "row" => {
                        if super::ooxml::attr(&e, "hidden").is_some_and(|v| v == "1" || v == "true")
                            && let Some(r) =
                                super::ooxml::attr(&e, "r").and_then(|r| r.parse().ok())
                        {
                            sheet.hidden_rows.insert(r);
                        }
                    }
                    "col" => {
                        if super::ooxml::attr(&e, "hidden").is_some_and(|v| v == "1" || v == "true")
                        {
                            let min: u32 = super::ooxml::attr(&e, "min")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0);
                            let max: u32 = super::ooxml::attr(&e, "max")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(min);
                            sheet.hidden_cols.extend(min..=max.min(min + 16_384));
                        }
                    }
                    "c" => {
                        if *budget == 0 {
                            sheet.truncated = true;
                            cell = None;
                            continue;
                        }
                        let position = super::ooxml::attr(&e, "r").and_then(|r| parse_ref(&r));
                        cell = position.map(|p| {
                            (
                                p,
                                Cell {
                                    kind: super::ooxml::attr(&e, "t").unwrap_or_else(|| "n".into()),
                                    ..Default::default()
                                },
                            )
                        });
                    }
                    "f" => {
                        field = Some("f");
                        buffer.clear();
                        // 共享公式：主单元格带文本与 si；从属单元格只有 si（Empty 事件）。
                        pending_shared = (super::ooxml::attr(&e, "t").as_deref() == Some("shared"))
                            .then(|| super::ooxml::attr(&e, "si"))
                            .flatten();
                        if let (Some(si), Some((_, c))) = (pending_shared.as_ref(), cell.as_mut())
                            && let Some(master) = shared.get(si)
                        {
                            c.formula = Some(master.clone());
                            c.formula_derived = true;
                        }
                        if self_closing {
                            field = None;
                            pending_shared = None;
                        }
                    }
                    "v" => {
                        field = Some("v");
                        buffer.clear();
                    }
                    "t" => {
                        field = Some("t");
                        buffer.clear();
                    }
                    "mergeCell" => {
                        if let Some(r) = super::ooxml::attr(&e, "ref") {
                            sheet.merged.push(r);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) if field.is_some() => buffer.push_str(&t),
            Event::GeneralRef(r) if field.is_some() => buffer.push_str(&entity(&r)),
            Event::End(e) => match e.local_name().as_ref() {
                "f" => {
                    if let Some((_, c)) = cell.as_mut()
                        && !buffer.is_empty()
                    {
                        c.formula = Some(buffer.clone());
                        c.formula_derived = false;
                        if let Some(si) = pending_shared.take() {
                            shared.insert(si, buffer.clone());
                        }
                    }
                    field = None;
                }
                "v" => {
                    if let Some((_, c)) = cell.as_mut() {
                        c.raw = Some(if c.kind == "s" {
                            buffer
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|i| strings.get(i).cloned())
                                .unwrap_or_default()
                        } else {
                            buffer.clone()
                        });
                    }
                    field = None;
                }
                "t" => {
                    if let Some((_, c)) = cell.as_mut() {
                        c.raw.get_or_insert_with(String::new).push_str(&buffer);
                    }
                    field = None;
                }
                "c" => {
                    if let Some((position, c)) = cell.take()
                        && (c.raw.as_ref().is_some_and(|v| !v.is_empty()) || c.formula.is_some())
                    {
                        *budget -= 1;
                        sheet.cells.insert(position, c);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sheet)
}
