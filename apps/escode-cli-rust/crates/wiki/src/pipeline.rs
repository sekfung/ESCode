//! 单文件导入管道的前半段（spec §6）：收集 → 安全校验 → 识别 → 解析（阻塞线程 + 截止时间）→ Document IR。
//! 只产出 IR，不写知识；写入在 apply.rs。
use super::ingest::{self, Ctx, Failure};
use escode_cli_domain::wiki::config::WikiConfig;
use escode_cli_domain::wiki::ir::{DocumentIr, ParseStatus};
use escode_cli_domain::wiki::text::{content_hash, sha256_hex};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_DIRECTORY_FILES: usize = 2_000;
const SKIPPED_DIRS: [&str; 6] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    ".escode",
    "__pycache__",
];

/// 展开路径参数：文件原样，目录递归收集支持的扩展名（不跟随目录符号链接，跳过构建/依赖目录）。
pub async fn collect(
    paths: &[PathBuf],
    wiki_roots: &[PathBuf],
) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut files = vec![];
    let mut rejected = vec![];
    // Global 根与项目内 `.escode/wiki/` 都是 Wiki 存储：其中的条目文件不能再被当作文档导入。
    let mut excluded = vec![];
    for root in wiki_roots {
        // 与 wiki 根的 realpath 同形态（host::realpath 去 Windows `\\?\` 前缀），否则 starts_with 比较会混形。
        excluded.push(
            escode_cli_host::realpath(root)
                .await
                .unwrap_or_else(|_| root.to_owned()),
        );
    }
    let mut queue: Vec<PathBuf> = paths.to_vec();
    while let Some(path) = queue.pop() {
        let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
            rejected.push((path, "path does not exist".into()));
            continue;
        };
        let real = escode_cli_host::realpath(&path)
            .await
            .unwrap_or_else(|_| path.clone());
        if excluded.iter().any(|root| real.starts_with(root)) {
            rejected.push((
                path,
                "files inside the wiki storage cannot be ingested".into(),
            ));
            continue;
        }
        if meta.is_dir() {
            let Ok(mut entries) = tokio::fs::read_dir(&path).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(kind) = entry.file_type().await else {
                    continue;
                };
                if kind.is_dir() && (SKIPPED_DIRS.contains(&name.as_str()) || name.starts_with('.'))
                {
                    continue;
                }
                if kind.is_dir() || (kind.is_file() && ingest::known_extension(&name)) {
                    queue.push(entry.path());
                }
            }
        } else if meta.is_file() || meta.file_type().is_symlink() {
            // 文件符号链接：按目标文件导入（目标必须是常规文件）。
            match tokio::fs::metadata(&path).await {
                Ok(target) if target.is_file() => files.push(path),
                _ => rejected.push((path, "not a regular file".into())),
            }
        } else {
            rejected.push((path, "not a regular file".into()));
        }
        if files.len() > MAX_DIRECTORY_FILES {
            rejected.push((
                PathBuf::from("…"),
                format!("more than {MAX_DIRECTORY_FILES} files; import a narrower directory"),
            ));
            files.truncate(MAX_DIRECTORY_FILES);
            break;
        }
    }
    files.sort();
    files.dedup();
    (files, rejected)
}

/// 来源 ID：项目根内按相对路径，项目外按规范化绝对路径（同一文件重复导入得到同一 ID）。
pub fn document_id(display_path: &str, inside_project: bool) -> String {
    let key = if inside_project {
        format!("rel:{display_path}")
    } else {
        format!("abs:{display_path}")
    };
    format!("doc-{}", &sha256_hex(key.as_bytes())[..16])
}

pub struct Read {
    pub bytes: Vec<u8>,
    pub hash: String,
    pub format: Option<&'static str>,
    pub warnings: Vec<String>,
}

pub async fn read(path: &Path, config: &WikiConfig) -> Result<Read, Failure> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| Failure::new("read", e.to_string()))?;
    if meta.len() > config.limits.max_file_bytes {
        return Err(Failure::limit(format!(
            "file is {} bytes, above wiki.limits.maxFileBytes ({})",
            meta.len(),
            config.limits.max_file_bytes
        )));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| Failure::new("read", e.to_string()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (format, warnings) = ingest::detect(&name, &bytes);
    Ok(Read {
        hash: content_hash(&bytes),
        bytes,
        format,
        warnings,
    })
}

/// 解析为 IR。DOC/XLS 先经转换器；解析在阻塞线程里执行并受截止时间约束。
pub async fn parse(
    read: Read,
    format: &'static str,
    config: &WikiConfig,
    document_id: &str,
    display_path: &str,
) -> Result<DocumentIr, Failure> {
    let (bytes, parse_as) = match format {
        "doc" | "xls" => {
            let converted =
                ingest::convert_legacy(&read.bytes, format, config.office_converter.as_ref())
                    .await?;
            (converted, if format == "doc" { "docx" } else { "xlsx" })
        }
        other => (read.bytes, other),
    };
    let limits = config.limits.clone();
    let max_ms = limits.max_parse_ms;
    let task =
        escode_cli_core_api::scope::Spawner::current().spawn_blocking("wiki-parse", move || {
            let ctx = Ctx {
                limits: &limits,
                deadline: Instant::now() + Duration::from_millis(max_ms),
            };
            ingest::parse(parse_as, &bytes, &ctx)
        });
    // 截止时间在解析器内分步检查；这里再加一道墙钟上限，防止单个步骤卡住阻塞导入批次。
    let parsed = match tokio::time::timeout(Duration::from_millis(max_ms + 5_000), task).await {
        Ok(Ok(result)) => result?,
        Ok(Err(join)) => return Err(Failure::parse(format!("parser crashed: {join}"))),
        Err(_) => {
            return Err(Failure::limit(format!(
                "parse time limit exceeded ({max_ms} ms)"
            )));
        }
    };
    let mut warnings = read.warnings;
    warnings.extend(parsed.warnings.iter().cloned());
    if format == "doc" || format == "xls" {
        warnings.push(format!("converted from legacy {format} with the configured office converter; layout fidelity depends on the converter"));
    }
    Ok(DocumentIr {
        document_id: document_id.to_owned(),
        source_path: display_path.to_owned(),
        source_format: format.to_owned(),
        content_hash: read.hash,
        document_title: parsed.title.clone(),
        document_metadata: parsed.metadata.clone(),
        parse_status: if parsed.degraded {
            ParseStatus::Degraded
        } else {
            parsed.status()
        },
        blocks: parsed.blocks,
        parser_name: ingest::parser_name(format),
        parser_version: ingest::PARSER_VERSION.to_owned(),
        warnings,
        confidence: parsed.confidence,
    })
}
