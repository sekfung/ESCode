//! Edit 的匹配结果落到文件内容，对齐 TS edit handler（docs/specs/rust-edit-matching.md）。

use anyhow::{Result, bail};

use super::edit_match::{MatchResult, Strategy, find, normalize_replacement};
use super::edit_quotes::preserve_quote_style;

pub(super) struct Applied {
    pub(super) content: String,
    pub(super) actual_old: String,
    pub(super) actual_new: String,
    pub(super) strategy: Strategy,
    pub(super) candidates: usize,
}

/// 匹配并替换，失败文案与 TS edit handler 一致（`raw_search` 为模型原始 old_string）。
pub(super) fn apply(
    content: &str,
    search: &str,
    replacement: &str,
    raw_search: &str,
    replace_all: bool,
) -> Result<Applied> {
    let (actual, strategy, candidates) = match find(content, search, replace_all) {
        MatchResult::Matched {
            actual,
            strategy,
            candidates,
        } => (actual, strategy, candidates),
        MatchResult::Ambiguous { candidates } => bail!(
            "edit_ambiguous_replace: {}",
            ambiguous_message(candidates, raw_search)
        ),
        MatchResult::NotFound => bail!(
            "edit_old_string_not_found: String to replace not found in file.\nString: {raw_search}"
        ),
    };
    let occurrences = content.matches(actual.as_str()).count();
    if !replace_all && occurrences > 1 {
        bail!(
            "edit_ambiguous_replace: {}",
            ambiguous_message(occurrences, raw_search)
        );
    }
    let actual_new = preserve_quote_style(
        search,
        &actual,
        &normalize_replacement(strategy, replacement),
    );
    // 与 TS applyEditToContent 一致：删除整行时连同其后的换行一起删除。
    let target = if actual_new.is_empty()
        && !actual.ends_with('\n')
        && content.contains(&format!("{actual}\n"))
    {
        format!("{actual}\n")
    } else {
        actual.clone()
    };
    let content = if replace_all {
        content.replace(&target, &actual_new)
    } else {
        content.replacen(&target, &actual_new, 1)
    };
    Ok(Applied {
        content,
        actual_old: actual,
        actual_new,
        strategy,
        candidates,
    })
}

/// 与 TS createAmbiguousEditMessage 逐字一致。
fn ambiguous_message(count: usize, old_string: &str) -> String {
    if count > 0 {
        format!(
            "Found {count} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance.
String: {old_string}"
        )
    } else {
        "old_string is not unique in the file. Provide more surrounding context or set replace_all to true.".to_owned()
    }
}
