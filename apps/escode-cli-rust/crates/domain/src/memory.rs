//! 项目记忆的纯规则（docs/specs/rust-project-memory.md）：记忆根目录名、Memory 段、MEMORY.md 索引格式化、
//! manifest 与提取提示词。逐条对齐 TS `core/src/memory/*` 与 `context/sections/{memory,request-user-context}.ts`。
use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub use super::memory_markdown::strip_top_level_html_comments;
pub use super::memory_policy::*;
pub use super::memory_stamp::stamp_origin;

const INDEX_LINE_LIMIT: usize = 200;
const INDEX_CHARACTER_LIMIT: usize = 25_000;
pub const MANIFEST_FILE_LIMIT: usize = 200;
pub const MANIFEST_PREVIEW_LINES: usize = 30;
pub const EXTRACTION_MAX_TURNS: usize = 5;

/// 记忆根的相对段 `memories/projects/<slug>-<hash16>/memory`（TS resolveProjectMemoryRoot）。
/// `workspace` 为规范化后的绝对路径，`name` 为其末段；Windows 由调用方传入已转小写的 key。
pub fn project_directory(identity: Option<&str>, key_path: &str, name: &str) -> String {
    let identity = identity.map(str::trim).filter(|s| !s.is_empty());
    let digest = Sha256::digest(identity.unwrap_or(key_path).as_bytes());
    let hash: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let slug = if identity.is_some() {
        "project".to_owned()
    } else {
        sanitize_slug(if name.is_empty() { "project" } else { name })
    };
    format!("{slug}-{}", &hash[..16])
}
fn sanitize_slug(value: &str) -> String {
    static INVALID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._-]+").unwrap());
    let lowered = value.to_lowercase();
    let replaced = INVALID.replace_all(&lowered, "-");
    let trimmed = replaced.trim_matches('-');
    let slug = String::from_utf16_lossy(&trimmed.encode_utf16().take(48).collect::<Vec<_>>());
    if slug.is_empty() {
        "project".into()
    } else {
        slug
    }
}

/// system 中的 Memory 段（TS buildMemorySection）。
pub fn section(memory_root: &str) -> String {
    static TEMPLATE: &str = include_str!("memory_section.md");
    TEMPLATE.trim_end().replace("{memoryRoot}", memory_root)
}

/// agentsMd 块中的索引段（TS buildProjectMemoryIndexContent）；`index_path` 为平台拼接后的 MEMORY.md 路径。
pub fn index_block(index_path: &str, content: &str) -> Option<String> {
    let formatted = format_project_index(content);
    (!formatted.is_empty()).then(|| {
        format!(
            "Contents of {index_path} (user's auto-memory, persists across conversations):\n\n{formatted}"
        )
    })
}

pub fn format_project_index(content: &str) -> String {
    static FRONTMATTER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^---\s*\n[\s\S]*?---\s*\n?").unwrap());
    let without = FRONTMATTER.replace(content, "");
    format_index(&strip_top_level_html_comments(&without))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}
fn format_index(content: &str) -> String {
    let trimmed = content.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if trimmed.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let line_count = lines.len();
    let characters = utf16_len(trimmed);
    let line_truncated = line_count > INDEX_LINE_LIMIT;
    let character_truncated = characters > INDEX_CHARACTER_LIMIT;
    if !line_truncated && !character_truncated {
        return trimmed.into();
    }
    let mut truncated: Vec<u16> = if line_truncated {
        lines[..INDEX_LINE_LIMIT]
            .join("\n")
            .encode_utf16()
            .collect()
    } else {
        trimmed.encode_utf16().collect()
    };
    if truncated.len() > INDEX_CHARACTER_LIMIT {
        // JS lastIndexOf("\n", LIMIT)：从 LIMIT 位置（含）向前找。
        let last = truncated[..=INDEX_CHARACTER_LIMIT]
            .iter()
            .rposition(|u| *u == u16::from(b'\n'));
        let end = last.filter(|i| *i > 0).unwrap_or(INDEX_CHARACTER_LIMIT);
        truncated.truncate(end);
    }
    let size = if character_truncated && !line_truncated {
        format!(
            "{} (limit: {}) \u{2014} index entries are too long",
            format_bytes(characters),
            format_bytes(INDEX_CHARACTER_LIMIT)
        )
    } else if line_truncated && !character_truncated {
        format!("{line_count} lines (limit: {INDEX_LINE_LIMIT})")
    } else {
        format!("{line_count} lines and {}", format_bytes(characters))
    };
    format!(
        "{}\n\n> WARNING: MEMORY.md is {size}. Only part of it was loaded. Keep index entries to one line under ~200 chars; move detail into topic files.",
        String::from_utf16_lossy(&truncated)
    )
}
/// JS `toFixed(1).replace(/\.0$/, "")`；`numerator / 2^shift` 为精确二进制值，toFixed 平局取较大值。
fn fixed1(numerator: usize, shift: u32) -> String {
    let tenths = ((numerator as u128 * 10) + (1u128 << (shift - 1))) >> shift;
    let (whole, fraction) = (tenths / 10, tenths % 10);
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}
fn format_bytes(value: usize) -> String {
    if value < 1024 {
        format!("{value} bytes")
    } else if value < 1024 * 1024 {
        format!("{}KB", fixed1(value, 10))
    } else if value < 1024 * 1024 * 1024 {
        format!("{}MB", fixed1(value, 20))
    } else {
        format!("{}GB", fixed1(value, 30))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManifestEntry {
    pub description: Option<String>,
    pub filename: String,
    pub mtime_ms: f64,
    pub kind: Option<String>,
}
/// 记忆文件 frontmatter 中的 description 与 type（metadata.type 优先），TS parseMemoryFrontmatter。
pub fn manifest_frontmatter(preview: &str) -> (Option<String>, Option<String>) {
    super::memory_yaml::description_and_type(preview)
}
pub fn format_manifest(entries: &[ManifestEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let kind = entry
                .kind
                .as_ref()
                .map(|t| format!("[{t}] "))
                .unwrap_or_default();
            let base = format!(
                "- {kind}{} ({})",
                entry.filename,
                iso_timestamp(entry.mtime_ms)
            );
            match &entry.description {
                Some(d) => format!("{base}: {d}"),
                None => base,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
/// JS `new Date(ms).toISOString()`。
pub fn iso_timestamp(ms: f64) -> String {
    let ms = ms.floor() as i64;
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // civil_from_days（Howard Hinnant）。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}
pub fn extraction_prompt(manifest: &[ManifestEntry], message_count: usize) -> String {
    let existing = if manifest.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Existing memory files\n\n{}\n\nCheck this list before writing \u{2014} update an existing file rather than creating a duplicate.",
            format_manifest(manifest)
        )
    };
    include_str!("memory_extraction_prompt.md")
        .trim_end()
        .replace("{count}", &message_count.to_string())
        .replace("{existing}", &existing)
}
