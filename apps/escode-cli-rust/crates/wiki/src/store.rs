//! 单个 scope（global 或某个项目）的持久化（spec §2–§3、§7）。条目文件是唯一事实来源；
//! 来源记录、IR 缓存与索引都是派生/辅助数据。写入顺序：条目 → 来源 → 索引 → 审计。
use super::fsio::{append_line, list_files, read_optional, write_atomic};
use anyhow::{Context, Result};
use escode_cli_domain::wiki::entry::{Entry, Scope, valid_id};
use escode_cli_domain::wiki::frontmatter;
use escode_cli_domain::wiki::index::{INDEX_FORMAT, Index, fingerprint};
use escode_cli_domain::wiki::ir::DocumentIr;
use escode_cli_domain::wiki::source::SourceRecord;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ScopeStore {
    pub root: PathBuf,
    pub scope: Scope,
    /// project scope 的项目 ID；global 为 None。条目的 project_id 必须与之相同。
    pub project_id: Option<String>,
}

/// 一次全量加载的结果；无法解析的条目文件单独列出，不静默丢弃（lint 报告）。
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub entries: Vec<Entry>,
    pub invalid: Vec<(String, String)>,
}

impl ScopeStore {
    pub fn global(root: &Path) -> Self {
        Self {
            root: root.join("global"),
            scope: Scope::Global,
            project_id: None,
        }
    }

    pub fn project(root: &Path, project_id: &str) -> Self {
        Self {
            root: root.join("projects").join(project_id),
            scope: Scope::Project,
            project_id: Some(project_id.to_owned()),
        }
    }

    fn entries_dir(&self) -> PathBuf {
        self.root.join("entries")
    }

    pub fn entry_path(&self, id: &str) -> Result<PathBuf> {
        // ID 只允许 [a-z0-9._-]：同时防止路径穿越（`..`、分隔符）进入存储目录之外。
        anyhow::ensure!(
            valid_id(id) && !id.contains(".."),
            "invalid wiki entry id '{id}'"
        );
        Ok(self.entries_dir().join(format!("{id}.md")))
    }

    pub async fn load(&self) -> Result<Loaded> {
        let mut loaded = Loaded::default();
        for path in list_files(&self.entries_dir(), "md").await? {
            let text = String::from_utf8_lossy(&tokio::fs::read(&path).await?).into_owned();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match frontmatter::parse(&text) {
                Ok(entry) if format!("{}.md", entry.meta.id) == name => loaded.entries.push(entry),
                Ok(entry) => loaded.invalid.push((
                    name,
                    format!("file name does not match id '{}'", entry.meta.id),
                )),
                Err(error) => loaded.invalid.push((name, error.to_string())),
            }
        }
        Ok(loaded)
    }

    pub async fn read_entry(&self, id: &str) -> Result<Option<Entry>> {
        let Some(bytes) = read_optional(&self.entry_path(id)?).await? else {
            return Ok(None);
        };
        let entry = frontmatter::parse(&String::from_utf8_lossy(&bytes))
            .map_err(|e| anyhow::anyhow!("wiki entry {id} is invalid: {e}"))?;
        Ok(Some(entry))
    }

    /// 写条目前做 schema 校验：不合格的条目绝不落盘（导入失败不得破坏已有知识）。
    pub async fn write_entry(&self, entry: &Entry) -> Result<()> {
        let issues =
            escode_cli_domain::wiki::entry::validate(&entry.meta, self.project_id.as_deref());
        anyhow::ensure!(
            issues.is_empty(),
            "refusing to write invalid wiki entry {}: {}",
            entry.meta.id,
            issues.join("; ")
        );
        write_atomic(
            &self.entry_path(&entry.meta.id)?,
            frontmatter::serialize(entry).as_bytes(),
        )
        .await
    }

    // ---------- 来源记录 ----------

    fn source_path(&self, document_id: &str) -> Result<PathBuf> {
        anyhow::ensure!(
            document_id.starts_with("doc-")
                && document_id[4..].bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid document id"
        );
        Ok(self
            .root
            .join("sources")
            .join(format!("{document_id}.json")))
    }

    pub async fn sources(&self) -> Result<Vec<SourceRecord>> {
        let mut out = vec![];
        for path in list_files(&self.root.join("sources"), "json").await? {
            if let Ok(record) =
                serde_json::from_slice::<SourceRecord>(&tokio::fs::read(&path).await?)
            {
                out.push(record);
            }
        }
        Ok(out)
    }

    pub async fn source(&self, document_id: &str) -> Result<Option<SourceRecord>> {
        let Some(bytes) = read_optional(&self.source_path(document_id)?).await? else {
            return Ok(None);
        };
        Ok(serde_json::from_slice(&bytes).ok())
    }

    pub async fn write_source(&self, record: &SourceRecord) -> Result<()> {
        write_atomic(
            &self.source_path(&record.document_id)?,
            &serde_json::to_vec_pretty(record)?,
        )
        .await
    }

    // ---------- IR 缓存（按内容哈希，可删除） ----------

    fn ir_path(&self, content_hash: &str) -> PathBuf {
        let hex: String = content_hash
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect();
        self.root.join("ir").join(format!("{hex}.json"))
    }

    pub async fn cached_ir(&self, content_hash: &str, parser_version: &str) -> Option<DocumentIr> {
        let bytes = read_optional(&self.ir_path(content_hash)).await.ok()??;
        let ir: DocumentIr = serde_json::from_slice(&bytes).ok()?;
        (ir.parser_version == parser_version).then_some(ir)
    }

    pub async fn write_ir(&self, ir: &DocumentIr) -> Result<()> {
        write_atomic(&self.ir_path(&ir.content_hash), &serde_json::to_vec(ir)?).await
    }

    // ---------- 索引 ----------

    /// 读索引；格式不符或指纹与条目不一致（崩溃、手工编辑）时从条目重建并落盘。
    /// 返回 (索引, 是否重建)。调用方需持有写锁才可落盘；只读路径传 `persist=false`。
    pub async fn index(&self, entries: &[Entry], persist: bool) -> Result<(Index, bool)> {
        let expected = fingerprint(
            entries
                .iter()
                .map(|e| (e.meta.id.as_str(), e.meta.updated_at.as_str())),
        );
        let path = self.root.join("index.json");
        if let Some(bytes) = read_optional(&path).await?
            && let Ok(index) = serde_json::from_slice::<Index>(&bytes)
            && index.format == INDEX_FORMAT
            && index.fingerprint == expected
        {
            return Ok((index, false));
        }
        let previous_version = read_optional(&path)
            .await?
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["version"].as_u64())
            .unwrap_or(0);
        let mut index = Index::build(entries.iter());
        index.version = previous_version + 1;
        if persist {
            self.write_index(&index).await?;
        }
        Ok((index, true))
    }

    pub async fn write_index(&self, index: &Index) -> Result<()> {
        write_atomic(&self.root.join("index.json"), &serde_json::to_vec(index)?).await
    }

    pub async fn index_version(&self) -> u64 {
        read_optional(&self.root.join("index.json"))
            .await
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v["version"].as_u64())
            .unwrap_or(0)
    }

    // ---------- 审计 ----------

    pub async fn audit(&self, event: Value) -> Result<()> {
        append_line(
            &self.root.join("audit.jsonl"),
            &serde_json::to_string(&event)?,
        )
        .await
    }

    pub async fn read_audit(&self, limit: usize) -> Result<Vec<Value>> {
        let Some(bytes) = read_optional(&self.root.join("audit.jsonl")).await? else {
            return Ok(vec![]);
        };
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        Ok(lines[lines.len().saturating_sub(limit)..]
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }

    pub async fn write_json(&self, name: &str, value: &Value) -> Result<()> {
        write_atomic(&self.root.join(name), &serde_json::to_vec_pretty(value)?)
            .await
            .with_context(|| format!("write {name}"))
    }
}
