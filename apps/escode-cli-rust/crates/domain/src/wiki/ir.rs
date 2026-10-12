//! Document IR：所有格式解析器的统一输出（spec §6）。解析器只产出 IR，不决定知识是否成为规范。
use super::entry::Locator;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseStatus {
    Ok,
    Degraded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    Heading,
    Paragraph,
    ListItem,
    Table,
    Code,
    Quote,
    Record,
    Field,
    Slide,
    Note,
    Footnote,
    HeaderFooter,
    Comment,
    ImageRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub order: usize,
    pub block_type: BlockType,
    /// 所属章节路径；抽取按「连续且路径相同」的块组成一个候选知识。
    pub heading_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    pub locator: Locator,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentIr {
    pub document_id: String,
    pub source_path: String,
    pub source_format: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_title: Option<String>,
    #[serde(default)]
    pub document_metadata: Map<String, Value>,
    pub blocks: Vec<Block>,
    pub parser_name: String,
    pub parser_version: String,
    pub parse_status: ParseStatus,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// 解析器向 IR 追加块的小工具：自动编号。
#[derive(Default)]
pub struct BlockSink {
    pub blocks: Vec<Block>,
}

impl BlockSink {
    pub fn push(
        &mut self,
        block_type: BlockType,
        heading_path: &[String],
        locator: Locator,
        text: impl Into<String>,
    ) -> &mut Block {
        let order = self.blocks.len();
        self.blocks.push(Block {
            order,
            block_type,
            heading_path: heading_path.to_vec(),
            level: None,
            locator,
            text: text.into(),
            raw_ref: None,
            lang: None,
        });
        self.blocks.last_mut().expect("just pushed")
    }
}

pub fn locator(kind: &str, value: impl Into<String>) -> Locator {
    Locator {
        kind: kind.to_owned(),
        value: value.into(),
    }
}
