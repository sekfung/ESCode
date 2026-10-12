//! 导入编排（spec §6、§7、§9）：持 scope 写锁，逐文件执行管道；单个文件失败不影响其他文件，
//! 也不改动已有知识。每个文件完成后立即写条目与来源记录；索引是派生数据，批次结束时写一次，
//! 中途崩溃时其指纹与条目不符，下次加载自动重建（store.index），因此重试仍幂等。
use super::apply::{Context, apply};
use super::fsio::ScopeLock;
use super::pipeline;
use super::report::{Item, Report};
use super::service::{WikiService, now};
use super::store::ScopeStore;
use anyhow::Result;
use escode_cli_domain::wiki::config::WikiConfig;
use escode_cli_domain::wiki::entry::Scope;
use escode_cli_domain::wiki::extract::extract;
use escode_cli_domain::wiki::index::Index;
use escode_cli_domain::wiki::source::{SourceRecord, SourceStatus, StageError};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

impl WikiService {
    pub async fn ingest(&self, paths: &[PathBuf], scope: Scope, approve: bool) -> Result<Report> {
        let settings = self.settings().await;
        anyhow::ensure!(
            settings.config.enabled,
            "wiki is disabled (wiki.enabled=false)"
        );
        let store = self.store_for(&settings, scope).await;
        let _lock = ScopeLock::acquire(&store.root).await?;
        let absolute: Vec<PathBuf> = paths
            .iter()
            .map(|p| {
                if p.is_absolute() {
                    p.clone()
                } else {
                    self.cwd.join(p)
                }
            })
            .collect();
        let (files, rejected) = pipeline::collect(&absolute, &settings.root).await;
        let mut report = Report {
            scope: scope.as_str().into(),
            items: vec![],
        };
        for (path, reason) in rejected {
            report.items.push(Item {
                path: path.display().to_string(),
                status: "skipped",
                error: Some(reason),
                ..Default::default()
            });
        }
        let loaded = store.load().await?;
        let (mut index, _) = store.index(&loaded.entries, true).await?;
        let mut index_changed = false;
        for file in files {
            let (item, changed) = self
                .ingest_file(&store, &settings.config, &file, approve, &mut index)
                .await?;
            index_changed |= changed;
            report.items.push(item);
        }
        // 修复原因：原先每个文件都整体重写 index.json，500 个文件时写入量随条目数平方增长（首导 >2 分钟）。
        if index_changed {
            store.write_index(&index).await?;
        }
        store.audit(json!({"event": "ingest", "at": now(), "scope": scope.as_str(), "approve": approve, "summary": report.summary()})).await?;
        self.invalidate().await;
        Ok(report)
    }

    /// 单文件导入；返回 Err 只表示存储写失败（批次中止），解析失败记在 Item 里。
    /// 第二个返回值表示内存索引是否变化，由调用方在批次结束时统一落盘。
    pub(crate) async fn ingest_file(
        &self,
        store: &ScopeStore,
        config: &WikiConfig,
        file: &Path,
        approve: bool,
        index: &mut Index,
    ) -> Result<(Item, bool)> {
        let started = Instant::now();
        let real = tokio::fs::canonicalize(file)
            .await
            .unwrap_or_else(|_| file.to_owned());
        let display = self.display_path(&real).await;
        let inside = !Path::new(&display).is_absolute() && store.scope == Scope::Project;
        let document_id = pipeline::document_id(&display, inside);
        let mut item = Item {
            path: display.clone(),
            document_id: Some(document_id.clone()),
            status: "failed",
            ..Default::default()
        };
        let previous = store.source(&document_id).await?;
        let finish = |mut item: Item| {
            item.duration_ms = started.elapsed().as_millis() as u64;
            item
        };
        let read = match pipeline::read(&real, config).await {
            Ok(read) => read,
            Err(failure) => {
                item.error = Some(format!("{}: {}", failure.stage, failure.message));
                self.record_failure(
                    store,
                    previous,
                    &document_id,
                    &display,
                    &real,
                    None,
                    &failure,
                )
                .await?;
                return Ok((finish(item), false));
            }
        };
        let Some(format) = read.format else {
            item.status = "skipped";
            item.error = Some("unsupported or unrecognized format".into());
            item.warnings = read.warnings;
            return Ok((finish(item), false));
        };
        item.format = Some(format.into());
        if !config.format_enabled(format) {
            item.status = "skipped";
            item.error = Some(format!(
                "format {format} is disabled by wiki.formats.{format}"
            ));
            return Ok((finish(item), false));
        }
        if let Some(prev) = &previous
            && prev.content_hash == read.hash
            && prev.status != SourceStatus::Failed
            && prev.parser_version == super::ingest::PARSER_VERSION
            && !approve
        {
            let mut record = prev.clone();
            record.status = SourceStatus::Current;
            record.checked_at = now();
            store.write_source(&record).await?;
            item.status = "unchanged";
            return Ok((finish(item), false));
        }
        let hash = read.hash.clone();
        let ir = match store
            .cached_ir(&hash, super::ingest::PARSER_VERSION)
            .await
            .filter(|ir| ir.document_id == document_id)
        {
            Some(mut ir) => {
                ir.source_path = display.clone();
                ir
            }
            None => match pipeline::parse(read, format, config, &document_id, &display).await {
                Ok(ir) => ir,
                Err(failure) => {
                    item.error = Some(format!("{}: {}", failure.stage, failure.message));
                    self.record_failure(
                        store,
                        previous,
                        &document_id,
                        &display,
                        &real,
                        Some((format, &hash)),
                        &failure,
                    )
                    .await?;
                    return Ok((finish(item), false));
                }
            },
        };
        store.write_ir(&ir).await?;
        item.warnings = ir.warnings.clone();
        let candidates = if let Some(existing) = existing_wiki_entry(&ir) {
            vec![existing]
        } else {
            extract(&ir)
        };
        let ctx = Context {
            store,
            ir: &ir,
            now: &now(),
            approve,
        };
        let previous_sections = previous
            .as_ref()
            .map(|p| p.sections.clone())
            .unwrap_or_default();
        let applied = apply(&ctx, &candidates, &previous_sections, &mut item).await?;
        let record = SourceRecord {
            document_id: document_id.clone(),
            scope: store.scope,
            source_path: display.clone(),
            absolute_path: real.to_string_lossy().into_owned(),
            format: format.into(),
            content_hash: hash,
            size: tokio::fs::metadata(&real)
                .await
                .map(|m| m.len())
                .unwrap_or(0),
            status: SourceStatus::Current,
            parser: ir.parser_name.clone(),
            parser_version: ir.parser_version.clone(),
            parse_status: format!("{:?}", ir.parse_status).to_lowercase(),
            warnings: ir.warnings.clone(),
            error: None,
            sections: applied.sections,
            previous_paths: previous
                .as_ref()
                .map(|p| p.previous_paths.clone())
                .unwrap_or_default(),
            imported_at: previous
                .as_ref()
                .map_or_else(now, |p| p.imported_at.clone()),
            checked_at: now(),
        };
        store.write_source(&record).await?;
        for entry in &applied.changed {
            index.upsert(entry);
        }
        item.status = match (previous.is_some(), ir.parse_status) {
            (_, escode_cli_domain::wiki::ir::ParseStatus::Degraded) => "degraded",
            (true, _) => "updated",
            (false, _) => "imported",
        };
        Ok((finish(item), !applied.changed.is_empty()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_failure(
        &self,
        store: &ScopeStore,
        previous: Option<SourceRecord>,
        document_id: &str,
        display: &str,
        real: &Path,
        read: Option<(&str, &str)>,
        failure: &super::ingest::Failure,
    ) -> Result<()> {
        // 失败只记在来源记录上；已有条目与其来源引用保持原样（解析失败不破坏已有 Wiki）。
        let mut record = previous.unwrap_or_else(|| SourceRecord {
            document_id: document_id.into(),
            scope: store.scope,
            source_path: display.into(),
            absolute_path: real.to_string_lossy().into_owned(),
            format: read.map_or("unknown", |r| r.0).into(),
            content_hash: read.map_or(String::new(), |r| r.1.to_owned()),
            size: 0,
            status: SourceStatus::Failed,
            parser: String::new(),
            parser_version: super::ingest::PARSER_VERSION.into(),
            parse_status: "failed".into(),
            warnings: vec![],
            error: None,
            sections: Default::default(),
            previous_paths: vec![],
            imported_at: now(),
            checked_at: now(),
        });
        record.status = SourceStatus::Failed;
        record.error = Some(StageError {
            stage: failure.stage.into(),
            message: failure.message.clone(),
        });
        record.checked_at = now();
        store.write_source(&record).await
    }
}

/// 已是 Wiki 条目格式的 Markdown：按原 ID 导入，避免重复生成（spec §7.7）。
fn existing_wiki_entry(
    ir: &escode_cli_domain::wiki::ir::DocumentIr,
) -> Option<escode_cli_domain::wiki::extract::Candidate> {
    let front = ir.document_metadata.get("frontmatter")?;
    let id = front.get("id")?.as_str()?;
    front.get("schema_version")?;
    let kind = front
        .get("kind")
        .and_then(|k| k.as_str())
        .and_then(escode_cli_domain::wiki::entry::Kind::parse)?;
    if !escode_cli_domain::wiki::entry::valid_id(id) {
        return None;
    }
    let body = ir
        .document_metadata
        .get("frontmatterBody")?
        .as_str()?
        .trim()
        .to_owned();
    Some(escode_cli_domain::wiki::extract::Candidate {
        id: id.to_owned(),
        title: front
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or(id)
            .to_owned(),
        kind,
        tags: front
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        excerpt: escode_cli_domain::wiki::text::truncate_chars(
            &body,
            escode_cli_domain::wiki::entry::EXCERPT_MAX_CHARS,
        ),
        body,
        locator: escode_cli_domain::wiki::ir::locator("lines", "1"),
        section_key: format!("entry:{id}"),
    })
}
