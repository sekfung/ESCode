//! 格式识别、安全校验与解析器路由（spec §6）。解析器是同步纯 CPU 函数：字节 → Document IR 片段，
//! 由调用方放进 `spawn_blocking`。每个解析器分步检查资源上限与截止时间，失败返回带阶段的错误。
mod csv;
mod docx;
mod json;
mod markdown;
mod office;
mod ooxml;
mod pdf;
mod pptx;
mod rst;
mod text;
mod xlsx;
mod xlsx_cells;
mod xml;
mod xmlutil;
mod yaml;

use escode_cli_domain::wiki::config::Limits;
use escode_cli_domain::wiki::ir::{Block, ParseStatus};
use serde_json::{Map, Value};
use std::time::Instant;

pub use office::convert_legacy;

pub const PARSER_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub stage: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
    pub fn parse(message: impl Into<String>) -> Self {
        Self::new("parse", message)
    }
    pub fn limit(message: impl Into<String>) -> Self {
        Self::new("security", message)
    }
}

pub type ParseResult = Result<Parsed, Failure>;

#[derive(Debug, Default)]
pub struct Parsed {
    pub title: Option<String>,
    pub metadata: Map<String, Value>,
    pub blocks: Vec<Block>,
    pub warnings: Vec<String>,
    pub degraded: bool,
    pub confidence: Option<f64>,
}

impl Parsed {
    pub fn status(&self) -> ParseStatus {
        if self.degraded {
            ParseStatus::Degraded
        } else {
            ParseStatus::Ok
        }
    }
}

pub struct Ctx<'a> {
    pub limits: &'a Limits,
    pub deadline: Instant,
}

impl Ctx<'_> {
    pub fn check_time(&self) -> Result<(), Failure> {
        if Instant::now() > self.deadline {
            return Err(Failure::limit(format!(
                "parse time limit exceeded ({} ms)",
                self.limits.max_parse_ms
            )));
        }
        Ok(())
    }
}

/// 仅按扩展名判断是否是可导入格式（目录收集用；真正识别在读取内容后进行）。
pub fn known_extension(file_name: &str) -> bool {
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "md" | "markdown"
            | "mdx"
            | "txt"
            | "text"
            | "csv"
            | "tsv"
            | "tab"
            | "json"
            | "yaml"
            | "yml"
            | "xml"
            | "docx"
            | "docm"
            | "doc"
            | "xlsx"
            | "xlsm"
            | "xls"
            | "pptx"
            | "pptm"
            | "pdf"
            | "rst"
            | "rest"
    )
}

/// 扩展名 + 魔数识别。返回 (格式名, 告警)。内容与扩展名不符时以内容为准。
pub fn detect(file_name: &str, bytes: &[u8]) -> (Option<&'static str>, Vec<String>) {
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let by_ext: Option<&'static str> = match ext.as_str() {
        "md" | "markdown" | "mdx" => Some("markdown"),
        "txt" | "text" | "log" => Some("txt"),
        "csv" => Some("csv"),
        "tsv" | "tab" => Some("tsv"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "xml" => Some("xml"),
        "docx" | "docm" => Some("docx"),
        "doc" => Some("doc"),
        "xlsx" | "xlsm" => Some("xlsx"),
        "xls" => Some("xls"),
        "pptx" | "pptm" => Some("pptx"),
        "pdf" => Some("pdf"),
        "rst" | "rest" => Some("rst"),
        _ => None,
    };
    let mut warnings = vec![];
    let by_magic: Option<&'static str> = if bytes.starts_with(b"%PDF-") {
        Some("pdf")
    } else if bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        Some(if by_ext == Some("xls") { "xls" } else { "doc" })
    } else if bytes.starts_with(b"PK\x03\x04") {
        ooxml::sniff(bytes)
    } else {
        None
    };
    match (by_ext, by_magic) {
        (Some(e), Some(m)) if e != m => {
            warnings.push(format!(
                "file extension says {e} but content is {m}; parsed as {m}"
            ));
            (Some(m), warnings)
        }
        (_, Some(m)) => (Some(m), warnings),
        (Some(e), None) if matches!(e, "pdf" | "docx" | "xlsx" | "pptx" | "doc" | "xls") => {
            warnings.push(format!(
                "file extension says {e} but the content signature does not match"
            ));
            (None, warnings)
        }
        (e, None) => (e, warnings),
    }
}

/// 按格式解析。`doc`/`xls` 不在这里处理（需要转换器，见 `office::convert_legacy`）。
pub fn parse(format: &str, bytes: &[u8], ctx: &Ctx<'_>) -> ParseResult {
    if bytes.len() as u64 > ctx.limits.max_file_bytes {
        return Err(Failure::limit(format!(
            "file is {} bytes, above wiki.limits.maxFileBytes ({})",
            bytes.len(),
            ctx.limits.max_file_bytes
        )));
    }
    match format {
        "markdown" => markdown::parse(&text::decode(bytes)?, ctx),
        "txt" => text::parse(&text::decode(bytes)?, ctx),
        "rst" => rst::parse(&text::decode(bytes)?, ctx),
        "csv" => csv::parse(&text::decode(bytes)?, b',', ctx),
        "tsv" => csv::parse(&text::decode(bytes)?, b'\t', ctx),
        "json" => json::parse(&text::decode(bytes)?, ctx),
        "yaml" => yaml::parse(&text::decode(bytes)?, ctx),
        "xml" => xml::parse(&text::decode(bytes)?, ctx),
        "docx" => docx::parse(bytes, ctx),
        "xlsx" => xlsx::parse(bytes, ctx),
        "pptx" => pptx::parse(bytes, ctx),
        "pdf" => pdf::parse(bytes, ctx),
        other => Err(Failure::new(
            "identify",
            format!("unsupported format {other}"),
        )),
    }
}

/// 解析器名称（进入 IR 与来源引用）。
pub fn parser_name(format: &str) -> String {
    match format {
        "doc" | "xls" => format!("{format}-via-office-converter"),
        other => other.to_owned(),
    }
}
