//! TS `buildSearchSnippets` 的等价实现（`listTaskList` 搜索摘要窗口）。
//!
//! 索引口径 = **UTF-16 码元**（JS `String.prototype.slice`/`indexOf` 的语义）：
//! 匹配在 `toLowerCase` 后的文本上做，但窗口切的是**原文**同一批码元下标——TS 也是这样
//! （lowercase 与原文长度不等时，双方有同一个怪癖）。上限/兜底规则见常量。
//!
//! 登记差异（见 docs/specs/rust-u2-tasks-index.md §3）：大小写折叠用 Rust `to_lowercase`
//! （全 Unicode 默认映射），TS 用 `toLocaleLowerCase()`（默认 locale）。只有在土耳其语等
//! 特殊 case mapping（如 `I`→`ı`）且 locale 命中时才可见差异。

const TASK_SEARCH_SNIPPET_PREFIX_RADIUS: usize = 20;
const TASK_SEARCH_SNIPPET_SUFFIX_RADIUS: usize = 72;
const TASK_SEARCH_SNIPPET_MAX_CHARS: usize = 140;
const TASK_SEARCH_SNIPPET_LIMIT: usize = 4;

/// TS `buildSearchSnippets`：窗口截取 + 近重叠合并 + 上限 4 条 + 兜底整段。
pub(super) fn build_search_snippets(searchable_text: &str, search: &str) -> Option<Vec<String>> {
    if searchable_text.trim().is_empty() {
        return None;
    }
    let search_units: Vec<u16> = search.to_lowercase().encode_utf16().collect();
    if search_units.is_empty() {
        return None;
    }
    let normalized_units: Vec<u16> = searchable_text.to_lowercase().encode_utf16().collect();
    let original_units: Vec<u16> = searchable_text.encode_utf16().collect();
    let total_units = original_units.len();

    let mut snippets: Vec<String> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut search_start = 0usize;
    while snippets.len() < TASK_SEARCH_SNIPPET_LIMIT && search_start < normalized_units.len() {
        let Some(match_index) = find_units(&normalized_units, &search_units, search_start) else {
            break;
        };
        let start = match_index.saturating_sub(TASK_SEARCH_SNIPPET_PREFIX_RADIUS);
        let end = (match_index + search_units.len() + TASK_SEARCH_SNIPPET_SUFFIX_RADIUS)
            .min(total_units);
        let mut raw = String::new();
        if start > 0 {
            raw.push_str("...");
        }
        raw.push_str(&String::from_utf16_lossy(&original_units[start..end]));
        if end < total_units {
            raw.push_str("...");
        }
        let snippet = normalize_snippet(&raw);
        let overlaps = ranges
            .iter()
            .any(|(range_start, range_end)| (*range_end).min(end) > (*range_start).max(start));
        if !snippet.is_empty() && !overlaps {
            snippets.push(snippet);
            ranges.push((start, end));
        }
        search_start = match_index + search_units.len();
    }
    if snippets.is_empty() {
        // title 命中但正文未命中：一条整段摘要兜底。
        let fallback = normalize_snippet(searchable_text);
        return (!fallback.is_empty()).then_some(vec![fallback]);
    }
    Some(snippets)
}

/// UTF-16 码元序列里从 `from` 开始找 `needle`（等价 `String#indexOf`）。
fn find_units(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len().saturating_sub(needle.len()))
        .find(|start| haystack[*start..*start + needle.len()] == *needle)
}

pub(super) fn normalize_snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // 截断按 UTF-16 码元（TS `slice(0, 140)`），避免与 JS 在星平面字符上分叉。
    let units: Vec<u16> = collapsed.encode_utf16().collect();
    String::from_utf16_lossy(&units[..units.len().min(TASK_SEARCH_SNIPPET_MAX_CHARS)])
}
