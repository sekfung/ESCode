//! OOXML（DOCX/XLSX/PPTX）容器：ZIP 识别与带上限的读取（spec §13.2 解压炸弹防护）。
//! 先按中央目录声明检查条目数、单条与总解压大小、压缩比，读取时再按实际字节限流；宏只告警，从不执行。
use super::{Ctx, Failure};
use std::io::{Cursor, Read};
use zip::ZipArchive;

pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let names: Vec<&str> = archive.file_names().collect();
    if names.contains(&"word/document.xml") {
        Some("docx")
    } else if names.contains(&"xl/workbook.xml") {
        Some("xlsx")
    } else if names.contains(&"ppt/presentation.xml") {
        Some("pptx")
    } else {
        None
    }
}

pub struct Package {
    archive: ZipArchive<Cursor<Vec<u8>>>,
    remaining: u64,
    pub warnings: Vec<String>,
}

impl Package {
    pub fn open(bytes: &[u8], ctx: &Ctx<'_>) -> Result<Self, Failure> {
        let mut archive = ZipArchive::new(Cursor::new(bytes.to_vec()))
            .map_err(|e| Failure::parse(format!("not a valid OOXML package: {e}")))?;
        let limits = ctx.limits;
        if archive.len() > limits.max_zip_entries {
            return Err(Failure::limit(format!(
                "package has {} entries (limit {})",
                archive.len(),
                limits.max_zip_entries
            )));
        }
        let mut total = 0u64;
        let mut warnings = vec![];
        for i in 0..archive.len() {
            let file = archive
                .by_index_raw(i)
                .map_err(|e| Failure::parse(e.to_string()))?;
            let size = file.size();
            let compressed = file.compressed_size().max(1);
            total = total.saturating_add(size);
            if size / compressed > limits.max_compression_ratio && size > 1024 * 1024 {
                return Err(Failure::limit(format!(
                    "entry {} has suspicious compression ratio {}",
                    file.name(),
                    size / compressed
                )));
            }
            let name = file.name().to_ascii_lowercase();
            if name.ends_with("vbaproject.bin") {
                warnings.push("document contains macros (vbaProject.bin); they are ignored and never executed".into());
            }
            if name.contains("embeddings/") || name.contains("activex") {
                warnings.push(format!("embedded object {} is not parsed", file.name()));
            }
        }
        if total > limits.max_uncompressed_bytes {
            return Err(Failure::limit(format!(
                "package expands to {total} bytes (limit {})",
                limits.max_uncompressed_bytes
            )));
        }
        warnings.dedup();
        Ok(Self {
            archive,
            remaining: limits.max_uncompressed_bytes,
            warnings,
        })
    }

    pub fn names(&self) -> Vec<String> {
        self.archive.file_names().map(str::to_owned).collect()
    }

    /// 读取一个条目为 UTF-8 文本；条目不存在返回 None。实际读取字节计入总预算。
    pub fn text(&mut self, name: &str) -> Result<Option<String>, Failure> {
        let mut file = match self.archive.by_name(name) {
            Ok(file) => file,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => return Err(Failure::parse(format!("{name}: {e}"))),
        };
        let mut bytes = vec![];
        let read = (&mut file)
            .take(self.remaining.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| Failure::parse(format!("{name}: {e}")))? as u64;
        if read > self.remaining {
            return Err(Failure::limit(
                "package content exceeds wiki.limits.maxUncompressedBytes",
            ));
        }
        self.remaining -= read;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// 关系文件（`_rels/*.rels`）：Id → Target。
    pub fn relationships(
        &mut self,
        rels: &str,
    ) -> Result<std::collections::HashMap<String, String>, Failure> {
        let mut map = std::collections::HashMap::new();
        let Some(text) = self.text(rels)? else {
            return Ok(map);
        };
        let mut reader = quick_xml::Reader::from_str(&text);
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e))
                    if e.local_name().as_ref() == "Relationship" =>
                {
                    let id = attr(&e, "Id");
                    let target = attr(&e, "Target");
                    if let (Some(id), Some(target)) = (id, target) {
                        map.insert(id, target);
                    }
                }
                Ok(quick_xml::events::Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        Ok(map)
    }
}

/// 读属性（按本地名，忽略命名空间前缀）。
pub fn attr(e: &quick_xml::events::BytesStart<'_>, local: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .map(|a| {
            a.normalized_value(quick_xml::XmlVersion::default())
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| a.value.to_string())
        })
}

/// 关系 ID（`r:id`）：只认带命名空间前缀的 `id` 属性。
/// 修复原因：`<p:sldId id="256" r:id="rId2"/>` 同时有无前缀的数字 id，按本地名取会拿错。
pub fn rel_id(e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.prefix().is_some() && a.key.local_name().as_ref() == "id")
        .map(|a| a.value.to_string())
}

/// 读取 docProps/core.xml 中的标题与作者等元数据。
pub fn core_properties(
    package: &mut Package,
) -> Result<serde_json::Map<String, serde_json::Value>, Failure> {
    let mut out = serde_json::Map::new();
    let Some(text) = package.text("docProps/core.xml")? else {
        return Ok(out);
    };
    let mut reader = quick_xml::Reader::from_str(&text);
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                current = Some(e.local_name().as_ref().to_owned())
            }
            Ok(quick_xml::events::Event::Text(t)) => {
                if let Some(key) = current.as_ref().filter(|k| {
                    matches!(
                        k.as_str(),
                        "title"
                            | "creator"
                            | "subject"
                            | "description"
                            | "lastModifiedBy"
                            | "created"
                            | "modified"
                            | "keywords"
                    )
                }) {
                    let value = t.to_string();
                    if !value.trim().is_empty() {
                        out.insert(key.clone(), value.trim().into());
                    }
                }
            }
            Ok(quick_xml::events::Event::End(_)) => current = None,
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Ok(out)
}
