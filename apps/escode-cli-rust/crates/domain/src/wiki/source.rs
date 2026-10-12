//! 来源记录（`sources/<document_id>.json`）：原始文档身份、哈希、解析状态与它生成的条目。
//! 与条目分层保存（spec §8.3）；来源删除/移动只改这里的状态，不删除条目。
use super::entry::Scope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// 最近一次检查时内容与记录哈希一致。
    Current,
    /// 文件仍在但内容变化，尚未重新导入。
    Changed,
    /// 文件不存在（删除或移动到无法识别的位置）。
    Missing,
    /// 最近一次解析/抽取失败；已有条目保持不变。
    Failed,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Changed => "changed",
            Self::Missing => "missing",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageError {
    pub stage: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub document_id: String,
    pub scope: Scope,
    /// 展示与定位用路径：项目内为相对项目根，否则为规范化绝对路径。
    pub source_path: String,
    /// 上次读取时的绝对路径（sync 读文件用）。
    pub absolute_path: String,
    pub format: String,
    pub content_hash: String,
    pub size: u64,
    pub status: SourceStatus,
    pub parser: String,
    pub parser_version: String,
    pub parse_status: String,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StageError>,
    /// 章节键 → 条目 ID：增量导入据此对上已有条目。
    #[serde(default)]
    pub sections: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_paths: Vec<String>,
    pub imported_at: String,
    pub checked_at: String,
}

impl SourceRecord {
    pub fn entry_ids(&self) -> impl Iterator<Item = &String> {
        self.sections.values()
    }
}
