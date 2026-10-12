//! 确定性知识抽取：Document IR → 候选知识（spec §6「抽取」）。不调用模型；
//! 候选 ID 由「来源 + 章节键」派生，章节内容变化不改变 ID，从而重复导入幂等、修改后能对上原条目。
use super::entry::{Kind, Locator};
use super::ir::{Block, BlockType, DocumentIr};
use super::text::{normalize_whitespace, short_hash, slug, truncate_chars};
use std::collections::HashMap;

pub const MAX_BODY_CHARS: usize = 12_000;
const MIN_CONTENT_CHARS: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub kind: Kind,
    pub tags: Vec<String>,
    pub body: String,
    pub locator: Locator,
    pub excerpt: String,
    /// 章节键（不含内容）：同一来源内稳定。
    pub section_key: String,
}

pub fn extract(ir: &DocumentIr) -> Vec<Candidate> {
    let doc_title = ir
        .document_title
        .clone()
        .unwrap_or_else(|| file_stem(&ir.source_path));
    let doc_slug = slug(&file_stem(&ir.source_path), 32);
    let mut groups: Vec<(Vec<String>, Vec<&Block>)> = vec![];
    for block in &ir.blocks {
        match groups.last_mut() {
            Some((path, blocks)) if *path == block.heading_path => blocks.push(block),
            _ => groups.push((block.heading_path.clone(), vec![block])),
        }
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = vec![];
    for (path, blocks) in groups {
        let content: Vec<&&Block> = blocks
            .iter()
            .filter(|b| b.block_type != BlockType::Heading)
            .collect();
        let text_len: usize = content.iter().map(|b| b.text.trim().chars().count()).sum();
        if text_len < MIN_CONTENT_CHARS {
            continue;
        }
        let base_key = path.join(" / ");
        let occurrence = seen.entry(base_key.clone()).or_insert(0);
        *occurrence += 1;
        let section_key = if *occurrence > 1 {
            format!("{base_key}#{occurrence}")
        } else {
            base_key.clone()
        };
        let title = section_title(&doc_title, &path);
        let body = render_body(&content);
        let kind = infer_kind(&title, &body, &ir.source_format, &content);
        let section_slug = slug(path.last().map_or("section", String::as_str), 40);
        let id = format!(
            "{doc_slug}-{section_slug}-{}",
            short_hash(&format!("{}|{section_key}", ir.document_id), 6)
        );
        let mut tags = vec![ir.source_format.clone()];
        for segment in path.iter().rev().take(3) {
            let s = slug(segment, 24);
            if s != "entry" && !tags.contains(&s) {
                tags.push(s);
            }
        }
        out.push(Candidate {
            id,
            title,
            kind,
            tags,
            excerpt: truncate_chars(
                &normalize_whitespace(&body),
                super::entry::EXCERPT_MAX_CHARS,
            ),
            locator: merge_locator(&blocks, &path),
            body,
            section_key,
        });
    }
    out
}

/// 「文档标题 — 章节路径」；章节路径首段与文档标题相同时不重复。
fn section_title(doc_title: &str, path: &[String]) -> String {
    let segments: Vec<&str> = path
        .iter()
        .map(String::as_str)
        .skip_while(|s| *s == doc_title)
        .collect();
    let title = if segments.is_empty() {
        doc_title.to_owned()
    } else {
        format!("{doc_title} — {}", segments.join(" / "))
    };
    title.chars().take(200).collect()
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .map_or(name, |(stem, _)| stem)
        .to_owned()
}

fn render_body(blocks: &[&&Block]) -> String {
    let mut parts: Vec<String> = vec![];
    for block in blocks {
        let text = block.text.trim_end();
        if text.trim().is_empty() {
            continue;
        }
        parts.push(match block.block_type {
            BlockType::Code => format!("```{}\n{}\n```", block.lang.as_deref().unwrap_or(""), text),
            BlockType::ListItem => format!("- {}", text.trim_start_matches(['-', '*', ' '])),
            BlockType::Quote => text
                .lines()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => text.to_owned(),
        });
    }
    let body = parts.join("\n\n");
    if body.chars().count() > MAX_BODY_CHARS {
        format!(
            "{}\n\n(truncated — read the source for the remainder)",
            truncate_chars(&body, MAX_BODY_CHARS)
        )
    } else {
        body
    }
}

/// 章节定位：同类型定位合并成范围（行号、页码），其余取首块定位并附章节路径。
fn merge_locator(blocks: &[&Block], path: &[String]) -> Locator {
    let first = &blocks[0].locator;
    let same = blocks.iter().all(|b| b.locator.kind == first.kind);
    let heading = path.join(" / ");
    match first.kind.as_str() {
        "lines" if same => {
            let start = blocks[0]
                .locator
                .value
                .split('-')
                .next()
                .unwrap_or("")
                .to_owned();
            let end = blocks
                .last()
                .map(|b| b.locator.value.rsplit('-').next().unwrap_or("").to_owned())
                .unwrap_or_default();
            let range = if start == end {
                start
            } else {
                format!("{start}-{end}")
            };
            if heading.is_empty() {
                Locator {
                    kind: "lines".into(),
                    value: range,
                }
            } else {
                Locator {
                    kind: "heading".into(),
                    value: format!("{heading} (lines {range})"),
                }
            }
        }
        "page" if same => {
            let start = &blocks[0].locator.value;
            let end = &blocks.last().unwrap().locator.value;
            let value = if start == end {
                format!("page {start}")
            } else {
                format!("pages {start}-{end}")
            };
            Locator {
                kind: "page".into(),
                value: if heading.is_empty() {
                    value
                } else {
                    format!("{value} — {heading}")
                },
            }
        }
        "paragraph" if !heading.is_empty() => Locator {
            kind: "heading".into(),
            value: heading,
        },
        _ => first.clone(),
    }
}

const NORMATIVE: &[&str] = &[
    "must",
    "must not",
    "shall",
    "shall not",
    "required",
    "never",
    "always",
    "do not",
    "don't",
    "should",
    "必须",
    "禁止",
    "不得",
    "不允许",
    "应当",
    "严禁",
    "务必",
    "不要",
];

fn count(text: &str, words: &[&str]) -> usize {
    words.iter().filter(|w| text.contains(*w)).count()
}

pub fn infer_kind(title: &str, body: &str, format: &str, blocks: &[&&Block]) -> Kind {
    let t = title.to_lowercase();
    let b = body.to_lowercase();
    let both = format!("{t}\n{b}");
    if t.contains("adr")
        || t.contains("decision")
        || t.contains("决策")
        || b.starts_with("status:") && b.contains("decision")
    {
        return Kind::Adr;
    }
    if t.contains("anti-pattern")
        || t.contains("antipattern")
        || t.contains("反模式")
        || t.contains("pitfall")
    {
        return Kind::AntiPattern;
    }
    if t.contains("invariant") || t.contains("不变量") || t.contains("不变式") {
        return Kind::Invariant;
    }
    let api = count(
        &both,
        &[
            "api",
            "endpoint",
            "request",
            "response",
            "schema",
            "interface",
            "protocol",
            "contract",
            "接口",
            "协议",
            "契约",
            "字段",
        ],
    );
    if t.contains("business")
        || t.contains("业务规则")
        || t.contains("业务") && count(&b, NORMATIVE) > 0
    {
        return Kind::BusinessRule;
    }
    if count(&both, NORMATIVE) >= 1 && api < 3 {
        return Kind::Standard;
    }
    if api >= 2 {
        return Kind::Contract;
    }
    if t.contains("pattern") || t.contains("模式") {
        return Kind::Pattern;
    }
    if t.contains("architecture")
        || t.contains("overview")
        || t.contains("module")
        || t.contains("架构")
        || t.contains("模块")
    {
        return Kind::Architecture;
    }
    if matches!(
        format,
        "csv" | "tsv" | "xlsx" | "xls" | "json" | "yaml" | "xml"
    ) || blocks.iter().all(|b| {
        matches!(
            b.block_type,
            BlockType::Table | BlockType::Record | BlockType::Field
        )
    }) {
        return Kind::Reference;
    }
    if count(
        &both,
        &[
            "how to", "step", "run ", "build", "deploy", "install", "步骤", "部署", "构建", "运行",
            "流程",
        ],
    ) > 0
    {
        return Kind::Guide;
    }
    Kind::Reference
}
