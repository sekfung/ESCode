//! 候选打分与 top-K（TS `packages/shared/src/workspaceFileSearch.ts`）。
//! 所有下标/长度都按 JS 字符串的 UTF-16 码元计，否则含 emoji/部分汉字的路径分值会与 TS 不同。

use crate::git::paths::js_trim;

pub const DISPLAY_CAP: usize = 1000;

/// 一个已归一化（trim + 小写）的候选。
pub struct Candidate {
    pub name: String,
    pub path: String,
    pub relative_path: String,
    pub directory: bool,
    lower_name: String,
    lower_relative: String,
    lower_path: String,
}

impl Candidate {
    pub fn new(name: String, path: String, relative_path: String, directory: bool) -> Self {
        // 与 TS `mapWorkspaceFileEntriesToSearchCandidates` 一致：先 trim 再小写。
        Self {
            lower_name: js_trim(&name).to_lowercase(),
            lower_relative: js_trim(&relative_path).to_lowercase(),
            lower_path: js_trim(&path).to_lowercase(),
            name,
            path,
            relative_path,
            directory,
        }
    }
}

fn utf16_len(text: &str) -> i64 {
    if text.is_ascii() {
        text.len() as i64
    } else {
        text.encode_utf16().count() as i64
    }
}

/// `scoreNormalizedFuzzyMatch`：`text` / `query` 均已 trim + 小写。
fn score_normalized(text: &str, query: &str) -> Option<i64> {
    if text.is_empty() {
        return None;
    }
    let (text_len, query_len) = (utf16_len(text), utf16_len(query));
    if text.starts_with(query) {
        return Some(text_len - query_len);
    }
    if let Some(byte_index) = text.find(query) {
        return Some(100 + utf16_len(&text[..byte_index]));
    }
    let mut score = 200_i64;
    let mut search_start = 0_i64;
    // 带 UTF-16 起点的字符表：`indexOf(char, searchStart)` 只在起点 ≥ searchStart 的位置匹配。
    let mut offsets: Vec<(i64, char)> = Vec::new();
    let mut unit = 0_i64;
    for c in text.chars() {
        offsets.push((unit, c));
        unit += c.len_utf16() as i64;
    }
    let mut cursor = 0_usize;
    for needle in query.chars() {
        let found = offsets[cursor..]
            .iter()
            .position(|(start, c)| *start >= search_start && *c == needle)?;
        let (found_unit, _) = offsets[cursor + found];
        score += found_unit - search_start;
        search_start = found_unit + 1;
        cursor += found;
    }
    Some(score + (text_len - query_len))
}

/// `getWorkspaceFileSearchCandidateScore`。
fn candidate_score(candidate: &Candidate, query: &str) -> Option<i64> {
    let name = score_normalized(&candidate.lower_name, query);
    let relative = score_normalized(&candidate.lower_relative, query);
    let path = score_normalized(&candidate.lower_path, query);
    let keyword = [relative.map(|s| s + 300), path.map(|s| s + 300)]
        .into_iter()
        .flatten()
        .min();
    [name, relative.map(|s| s + 25), keyword]
        .into_iter()
        .flatten()
        .min()
}

/// `filterWorkspaceFileSearchCandidates` + `searchHostFileCandidates` 的合并等价：
/// 按 `(score, 原索引)` 升序取前 `limit`；空 query 时目录排后、原序稳定。
pub fn top_k<'a>(candidates: &'a [Candidate], query: &str, limit: usize) -> Vec<&'a Candidate> {
    let normalized = js_trim(query).to_lowercase();
    if normalized.is_empty() {
        let mut files: Vec<&Candidate> = candidates.iter().filter(|c| !c.directory).collect();
        files.extend(candidates.iter().filter(|c| c.directory));
        files.truncate(limit);
        return files;
    }
    let mut scored: Vec<(i64, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            candidate_score(candidate, &normalized).map(|s| (s, index))
        })
        .collect();
    scored.sort_unstable();
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(_, index)| &candidates[index])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(relative: &str, directory: bool) -> Candidate {
        let name = relative.rsplit('/').next().unwrap().to_owned();
        Candidate::new(
            name,
            format!("/r/{relative}"),
            relative.to_owned(),
            directory,
        )
    }

    #[test]
    fn scoring_tiers_follow_the_ts_formula() {
        assert_eq!(score_normalized("readme.md", "read"), Some(5));
        assert_eq!(score_normalized("src/readme", "read"), Some(104));
        assert_eq!(score_normalized("abc", "ac"), Some(200 + 1 + 1));
        assert_eq!(score_normalized("abc", "d"), None);
        assert_eq!(score_normalized("", "a"), None);
    }

    #[test]
    fn indices_count_utf16_code_units() {
        // 😀 占两个码元：子串偏移与子序列间隔都要按码元计。
        assert_eq!(score_normalized("😀ab", "ab"), Some(100 + 2));
        assert_eq!(
            score_normalized("😀a😀b", "ab"),
            Some(200 + 2 + 2 + (6 - 2))
        );
        assert_eq!(utf16_len("中😀"), 3);
    }

    #[test]
    fn top_k_is_stable_and_empty_query_puts_directories_last() {
        let items = vec![
            candidate("b/z.ts", false),
            candidate("a", true),
            candidate("a/z.ts", false),
        ];
        let hits: Vec<&str> = top_k(&items, "z.ts", 10)
            .iter()
            .map(|c| c.relative_path.as_str())
            .collect();
        assert_eq!(hits, vec!["b/z.ts", "a/z.ts"], "同分按原索引");
        let empty: Vec<&str> = top_k(&items, "  ", 10)
            .iter()
            .map(|c| c.relative_path.as_str())
            .collect();
        assert_eq!(empty, vec!["b/z.ts", "a/z.ts", "a"]);
        assert_eq!(top_k(&items, "z", 1).len(), 1);
    }
}
