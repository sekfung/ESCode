//! 增量同步（spec §6「删除/移动」、§9）：检查已知来源的内容哈希；变化的重新导入，
//! 缺失的先在项目根内找同哈希文件（移动/重命名，保持 document_id），找不到则标 missing，
//! 关联条目标 `broken-source`——从不删除知识。
use super::fsio::ScopeLock;
use super::report::{Item, Report};
use super::service::{WikiService, now};
use super::store::ScopeStore;
use anyhow::Result;
use escode_cli_domain::wiki::entry::{Scope, Validity, ValidityInfo};
use escode_cli_domain::wiki::source::{SourceRecord, SourceStatus};
use escode_cli_domain::wiki::text::content_hash;
use serde_json::json;
use std::path::{Path, PathBuf};

const MOVE_SEARCH_LIMIT: usize = 5_000;

impl WikiService {
    pub async fn sync(&self, scope: Scope) -> Result<Report> {
        let settings = self.settings().await;
        anyhow::ensure!(
            settings.config.enabled,
            "wiki is disabled (wiki.enabled=false)"
        );
        let store = self.store_for(&settings, scope).await;
        let _lock = ScopeLock::acquire(&store.root).await?;
        let loaded = store.load().await?;
        let (mut index, _) = store.index(&loaded.entries, true).await?;
        let mut report = Report {
            scope: scope.as_str().into(),
            items: vec![],
        };
        for mut record in store.sources().await? {
            let path = PathBuf::from(&record.absolute_path);
            let current = match tokio::fs::read(&path).await {
                Ok(bytes) => Some(content_hash(&bytes)),
                Err(_) => None,
            };
            match current {
                Some(hash)
                    if hash == record.content_hash && record.status != SourceStatus::Failed =>
                {
                    record.status = SourceStatus::Current;
                    record.checked_at = now();
                    store.write_source(&record).await?;
                    report.items.push(Item {
                        path: record.source_path.clone(),
                        document_id: Some(record.document_id.clone()),
                        status: "unchanged",
                        ..Default::default()
                    });
                }
                Some(_) => {
                    let item = self
                        .ingest_file(&store, &settings.config, &path, false, &mut index)
                        .await?;
                    report.items.push(item);
                }
                None => {
                    let item = self.missing_source(&store, &mut record, &mut index).await?;
                    report.items.push(item);
                }
            }
        }
        store.audit(json!({"event": "sync", "at": now(), "scope": scope.as_str(), "summary": report.summary()})).await?;
        self.invalidate().await;
        Ok(report)
    }

    async fn missing_source(
        &self,
        store: &ScopeStore,
        record: &mut SourceRecord,
        index: &mut escode_cli_domain::wiki::index::Index,
    ) -> Result<Item> {
        let mut item = Item {
            path: record.source_path.clone(),
            document_id: Some(record.document_id.clone()),
            ..Default::default()
        };
        let root = PathBuf::from(&self.project().await.root);
        if let Some(moved) = find_moved(
            &root,
            &record.absolute_path,
            &record.content_hash,
            record.size,
        )
        .await
        {
            let display = self.display_path(&moved).await;
            record.previous_paths.push(record.source_path.clone());
            record.source_path = display.clone();
            record.absolute_path = moved.to_string_lossy().into_owned();
            record.status = SourceStatus::Current;
            record.checked_at = now();
            for id in record.sections.values() {
                if let Some(mut entry) = store.read_entry(id).await? {
                    for source in entry
                        .meta
                        .sources
                        .iter_mut()
                        .filter(|s| s.document_id == record.document_id)
                    {
                        source.source_path = display.clone();
                    }
                    if entry.meta.validity.state == Validity::BrokenSource {
                        entry.meta.validity = ValidityInfo {
                            state: Validity::Valid,
                            reason: None,
                        };
                    }
                    entry.meta.updated_at = now();
                    store.write_entry(&entry).await?;
                    index.upsert(&entry);
                }
            }
            store.write_source(record).await?;
            store.write_index(index).await?;
            item.status = "moved";
            item.path = display;
            return Ok(item);
        }
        record.status = SourceStatus::Missing;
        record.checked_at = now();
        for id in record.sections.values() {
            let Some(mut entry) = store.read_entry(id).await? else {
                continue;
            };
            if entry.retired() || entry.meta.validity.state == Validity::BrokenSource {
                continue;
            }
            entry.meta.validity = ValidityInfo {
                state: Validity::BrokenSource,
                reason: Some(format!(
                    "source {} was deleted or moved outside the project",
                    record.source_path
                )),
            };
            entry.meta.updated_at = now();
            store.write_entry(&entry).await?;
            index.upsert(&entry);
            item.staled += 1;
        }
        store.write_source(record).await?;
        store.write_index(index).await?;
        item.status = "missing";
        Ok(item)
    }
}

/// 在项目根内找同名或同内容的文件（有界遍历，跳过依赖/构建目录）。
async fn find_moved(root: &Path, old: &str, hash: &str, size: u64) -> Option<PathBuf> {
    let name = Path::new(old)
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let mut queue = vec![root.to_owned()];
    let mut seen = 0usize;
    while let Some(dir) = queue.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            seen += 1;
            if seen > MOVE_SEARCH_LIMIT {
                return None;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            if kind.is_dir() {
                if !file_name.starts_with('.')
                    && !matches!(file_name.as_str(), "node_modules" | "target" | "dist")
                {
                    queue.push(entry.path());
                }
                continue;
            }
            let same_ext = entry
                .path()
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                == name;
            // 先比大小再算哈希：只有大小相同的候选才读全文。
            let same_size = entry.metadata().await.is_ok_and(|m| m.len() == size);
            if kind.is_file()
                && same_ext
                && same_size
                && let Ok(bytes) = tokio::fs::read(entry.path()).await
                && content_hash(&bytes) == hash
            {
                return Some(entry.path());
            }
        }
    }
    None
}
