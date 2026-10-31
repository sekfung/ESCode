//! TS `readTextFileForModel` 的 token 预算与部分视图（docs/specs/rust-file-tool-results.md）。
use super::text_file::{PARTIAL_TARGET, READ_DEFAULT_MAX_LINES, READ_MAX_OUTPUT_TOKENS};

pub(super) struct Partial {
    pub(super) content: String,
    pub(super) num_lines: u64,
    pub(super) notice: String,
}

pub(super) fn partial_view(
    content: &str,
    start_line: u64,
    total_lines: u64,
    token_count: usize,
) -> Partial {
    let lines: Vec<&str> = content.split('\n').collect();
    let count = largest_prefix(lines.len(), |n| {
        estimate_tokens(&lines[..n].join("\n")) <= PARTIAL_TARGET
    });
    let head = format!(
        "The file is too large to display in full ({token_count} estimated tokens, limit {READ_MAX_OUTPUT_TOKENS})."
    );
    if count > 0 {
        let end_line = start_line + count as u64 - 1;
        return Partial {
            content: lines[..count].join("\n"),
            num_lines: count as u64,
            notice: format!(
                "{head} Showing a partial view of lines {start_line}-{end_line} of {total_lines}. Use Read with offset {} and limit {READ_DEFAULT_MAX_LINES} to continue, or use a search tool to find a specific section.",
                end_line + 1
            ),
        };
    }
    // 首行本身超出预算：TS 按 UTF-16 下标截取；这里按字符边界取不超过同一预算的最长前缀。
    let chars: Vec<char> = content.chars().collect();
    let count = largest_prefix(chars.len(), |n| {
        estimate_tokens(&chars[..n].iter().collect::<String>()) <= PARTIAL_TARGET
    });
    Partial {
        content: chars[..count].iter().collect(),
        num_lines: 1,
        notice: format!(
            "{head} Showing a partial view of the first line because the first line alone exceeds the token budget. Use Read with a smaller range or use a search tool to find a specific section."
        ),
    }
}

/// TS `estimateTokens`：CJK `[一-鿿]` 计 2，其余按 UTF-16 码元计，除以 3 向上取整。
pub(super) fn estimate_tokens(text: &str) -> usize {
    let mut weighted = 0usize;
    for c in text.chars() {
        weighted += if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            2
        } else {
            c.len_utf16()
        };
    }
    weighted.div_ceil(3)
}

/// TS `findLargestPrefixWithinTokenBudget` 的二分。
fn largest_prefix(len: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_follow_shared_estimate() {
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 2);
        assert_eq!(estimate_tokens("中"), 1);
        assert_eq!(estimate_tokens("中文"), 2);
        assert_eq!(estimate_tokens("😀"), 1);
    }
}
