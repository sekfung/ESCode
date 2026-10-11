//! Edit 的 old_string 宽松匹配，逐条对齐 TS `core/src/tool/edit-matchers.ts`。
//! 规则见 docs/specs/rust-edit-matching.md。

use super::edit_quotes::{LEFT_DOUBLE, LEFT_SINGLE, RIGHT_DOUBLE, RIGHT_SINGLE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Strategy {
    Exact,
    QuoteNormalized,
    LineNumberPrefixStripped,
    EscapeNormalized,
    UnicodeEscapeNormalized,
    LineTrimmed,
    IndentationFlexible,
    BlockAnchor,
}

impl Strategy {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::QuoteNormalized => "quote_normalized",
            Self::LineNumberPrefixStripped => "line_number_prefix_stripped",
            Self::EscapeNormalized => "escape_normalized",
            Self::UnicodeEscapeNormalized => "unicode_escape_normalized",
            Self::LineTrimmed => "line_trimmed",
            Self::IndentationFlexible => "indentation_flexible",
            Self::BlockAnchor => "block_anchor",
        }
    }

    fn broad(self) -> bool {
        matches!(
            self,
            Self::LineTrimmed | Self::IndentationFlexible | Self::BlockAnchor
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MatchResult {
    Matched {
        actual: String,
        strategy: Strategy,
        candidates: usize,
    },
    Ambiguous {
        candidates: usize,
    },
    NotFound,
}

const FALLBACKS: [Strategy; 7] = [
    Strategy::QuoteNormalized,
    Strategy::LineNumberPrefixStripped,
    Strategy::EscapeNormalized,
    Strategy::UnicodeEscapeNormalized,
    Strategy::LineTrimmed,
    Strategy::IndentationFlexible,
    Strategy::BlockAnchor,
];
const BLOCK_ANCHOR_MIN_SIMILARITY: f64 = 0.8;

pub(super) fn find(content: &str, search: &str, replace_all: bool) -> MatchResult {
    let exact = substring_candidates(content, search);
    if !exact.is_empty() {
        return to_result(Strategy::Exact, exact);
    }
    for strategy in FALLBACKS {
        if replace_all && strategy.broad() {
            continue;
        }
        let candidates = collect(strategy, content, search);
        if !candidates.is_empty() {
            return to_result(strategy, candidates);
        }
    }
    MatchResult::NotFound
}

/// escape_normalized 命中时 new_string 同样反转义，其余策略原样使用。
pub(super) fn normalize_replacement(strategy: Strategy, replacement: &str) -> String {
    if strategy == Strategy::EscapeNormalized {
        unescape_visible(replacement)
    } else {
        replacement.to_owned()
    }
}

fn collect(strategy: Strategy, content: &str, search: &str) -> Vec<String> {
    match strategy {
        Strategy::Exact => substring_candidates(content, search),
        Strategy::QuoteNormalized => quote_normalized_candidates(content, search),
        Strategy::LineNumberPrefixStripped => match strip_line_number_prefixes(search) {
            Some(stripped) if stripped != search => substring_candidates(content, &stripped),
            _ => Vec::new(),
        },
        Strategy::EscapeNormalized => {
            let unescaped = unescape_visible(search);
            if unescaped == search {
                Vec::new()
            } else {
                substring_candidates(content, &unescaped)
            }
        }
        Strategy::UnicodeEscapeNormalized => {
            // 允许 old_string 里的 \uXXXX 匹配文件中的真实字符，写入时用文件里的真实片段。
            let unescaped = unescape_unicode(search);
            if unescaped == search {
                Vec::new()
            } else {
                substring_candidates(content, &unescaped)
            }
        }
        Strategy::LineTrimmed => line_blocks(content, search, 1, |block, lines| {
            block
                .iter()
                .zip(lines)
                .all(|(line, expected)| js_trim(line) == js_trim(expected))
        }),
        Strategy::IndentationFlexible => {
            let lines = search_lines(search);
            let expected = remove_common_indent(&lines);
            line_blocks(content, search, 2, |block, _| {
                remove_common_indent(block) == expected
            })
        }
        Strategy::BlockAnchor => line_blocks(content, search, 3, |block, lines| {
            js_trim(block[0]) == js_trim(lines[0])
                && js_trim(block[block.len() - 1]) == js_trim(lines[lines.len() - 1])
                && average_middle_similarity(block, lines) >= BLOCK_ANCHOR_MIN_SIMILARITY
        }),
    }
}

fn to_result(strategy: Strategy, candidates: Vec<String>) -> MatchResult {
    let first = &candidates[0];
    if candidates.iter().any(|value| value != first) {
        return MatchResult::Ambiguous {
            candidates: candidates.len(),
        };
    }
    MatchResult::Matched {
        actual: first.clone(),
        strategy,
        candidates: candidates.len(),
    }
}

/// 不重叠的子串出现（与 TS `indexOf` 循环一致）。
fn substring_candidates(content: &str, search: &str) -> Vec<String> {
    if search.is_empty() {
        return Vec::new();
    }
    content
        .match_indices(search)
        .map(|(_, value)| value.to_owned())
        .collect()
}

/// 在引号归一化后的文本里查找，再映射回原文件片段；归一化逐字符替换，字符数不变。
fn quote_normalized_candidates(content: &str, search: &str) -> Vec<String> {
    let normalized_search = normalize_quotes(search);
    if normalized_search.is_empty() {
        return Vec::new();
    }
    let mut normalized = String::with_capacity(content.len());
    // normalized 的字节偏移 → content 的字节偏移（仅字符边界有效，末尾追加哨兵）。
    let mut offsets = vec![0; content.len() + 1];
    for (original, ch) in content.char_indices() {
        offsets[normalized.len()] = original;
        normalized.push(normalize_quote(ch));
    }
    offsets[normalized.len()] = content.len();
    normalized
        .match_indices(&normalized_search)
        .map(|(index, value)| content[offsets[index]..offsets[index + value.len()]].to_owned())
        .collect()
}

fn search_lines(search: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = search.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn line_blocks(
    content: &str,
    search: &str,
    min_lines: usize,
    accept: impl Fn(&[&str], &[&str]) -> bool,
) -> Vec<String> {
    let lines = search_lines(search);
    if lines.len() < min_lines {
        return Vec::new();
    }
    let content_lines: Vec<&str> = content.split('\n').collect();
    if content_lines.len() < lines.len() {
        return Vec::new();
    }
    (0..=content_lines.len() - lines.len())
        .filter_map(|index| {
            let block = &content_lines[index..index + lines.len()];
            accept(block, &lines).then(|| block.join("\n"))
        })
        .collect()
}

fn strip_line_number_prefixes(search: &str) -> Option<String> {
    search
        .split('\n')
        .map(|line| {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let rest = &line[digits..];
            rest.strip_prefix(": ").or_else(|| rest.strip_prefix('\t'))
        })
        .collect::<Option<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

fn unescape_visible(search: &str) -> String {
    let mut out = String::with_capacity(search.len());
    let mut chars = search.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let mapped = match chars.peek() {
                Some('n') => Some('\n'),
                Some('t') => Some('\t'),
                Some('r') => Some('\r'),
                Some(&c @ ('"' | '\'' | '`' | '\\' | '$')) => Some(c),
                _ => None,
            };
            if let Some(mapped) = mapped {
                chars.next();
                out.push(mapped);
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// `\\` 原样保留，`\uXXXX` 转为 UTF-16 码元（代理对可组合成一个字符）。
fn unescape_unicode(search: &str) -> String {
    let chars: Vec<char> = search.chars().collect();
    let mut units: Vec<u16> = Vec::with_capacity(search.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' && chars.get(index + 1) == Some(&'\\') {
            units.extend_from_slice(&[b'\\' as u16, b'\\' as u16]);
            index += 2;
            continue;
        }
        if chars[index] == '\\' && chars.get(index + 1) == Some(&'u') {
            let hex: String = chars.iter().skip(index + 2).take(4).collect();
            if hex.len() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                units.push(u16::from_str_radix(&hex, 16).unwrap_or_default());
                index += 6;
                continue;
            }
        }
        let mut buffer = [0; 2];
        units.extend_from_slice(chars[index].encode_utf16(&mut buffer));
        index += 1;
    }
    String::from_utf16_lossy(&units)
}

/// JS `String.prototype.trim`：Unicode 空白、行终止符与 BOM。
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

fn remove_common_indent(lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|line| !js_trim(line).is_empty())
        .map(|line| {
            line.bytes()
                .take_while(|b| *b == b' ' || *b == b'\t')
                .count()
        })
        .min();
    let Some(indent) = indent else {
        return lines.join("\n");
    };
    lines
        .iter()
        .map(|line| {
            if js_trim(line).is_empty() {
                *line
            } else {
                &line[indent..]
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn average_middle_similarity(actual: &[&str], expected: &[&str]) -> f64 {
    if actual.len() <= 2 {
        return 1.0;
    }
    let middle = 1..actual.len() - 1;
    let count = middle.len() as f64;
    let total: f64 = middle
        .map(|index| line_similarity(js_trim(actual[index]), js_trim(expected[index])))
        .sum();
    total / count
}

/// 以 UTF-16 码元计长度与编辑距离，与 TS 字符串语义一致。
fn line_similarity(left: &str, right: &str) -> f64 {
    if left == right {
        return 1.0;
    }
    let left: Vec<u16> = left.encode_utf16().collect();
    let right: Vec<u16> = right.encode_utf16().collect();
    let max = left.len().max(right.len());
    if max == 0 {
        return 1.0;
    }
    1.0 - levenshtein(&left, &right) as f64 / max as f64
}

fn levenshtein(left: &[u16], right: &[u16]) -> usize {
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (i, l) in left.iter().enumerate() {
        let mut current = vec![i + 1; right.len() + 1];
        for (j, r) in right.iter().enumerate() {
            let cost = usize::from(l != r);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        previous = current;
    }
    previous[right.len()]
}

fn normalize_quote(ch: char) -> char {
    match ch {
        LEFT_SINGLE | RIGHT_SINGLE => '\'',
        LEFT_DOUBLE | RIGHT_DOUBLE => '"',
        other => other,
    }
}

fn normalize_quotes(value: &str) -> String {
    value.chars().map(normalize_quote).collect()
}

#[cfg(test)]
#[path = "edit_match_tests.rs"]
mod tests;
