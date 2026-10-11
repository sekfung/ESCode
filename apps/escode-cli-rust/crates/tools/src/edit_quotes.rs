//! Edit 弯引号风格保留，对齐 TS `edit-matchers.ts` 的 preserveQuoteStyle。

use std::sync::LazyLock;

use regex::Regex;

pub(super) const LEFT_SINGLE: char = '\u{2018}';
pub(super) const RIGHT_SINGLE: char = '\u{2019}';
pub(super) const LEFT_DOUBLE: char = '\u{201c}';
pub(super) const RIGHT_DOUBLE: char = '\u{201d}';

pub(super) fn preserve_quote_style(old: &str, actual: &str, replacement: &str) -> String {
    if old == actual {
        return replacement.to_owned();
    }
    let mut result = replacement.to_owned();
    if actual.contains(LEFT_DOUBLE) || actual.contains(RIGHT_DOUBLE) {
        result = curly_double(&result);
    }
    if actual.contains(LEFT_SINGLE) || actual.contains(RIGHT_SINGLE) {
        result = curly_single(&result);
    }
    result
}

fn curly_double(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    (0..chars.len())
        .map(|index| match chars[index] {
            '"' if opening_context(&chars, index) => LEFT_DOUBLE,
            '"' => RIGHT_DOUBLE,
            other => other,
        })
        .collect()
}

fn curly_single(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    (0..chars.len())
        .map(|index| {
            if chars[index] != '\'' {
                return chars[index];
            }
            let previous = index.checked_sub(1).map(|i| chars[i]);
            if is_letter(previous) && is_letter(chars.get(index + 1).copied()) {
                return RIGHT_SINGLE;
            }
            if opening_context(&chars, index) {
                LEFT_SINGLE
            } else {
                RIGHT_SINGLE
            }
        })
        .collect()
}

fn opening_context(chars: &[char], index: usize) -> bool {
    index == 0
        || matches!(
            chars[index - 1],
            ' ' | '\t' | '\n' | '\r' | '(' | '[' | '{' | '\u{2014}' | '\u{2013}'
        )
}

fn is_letter(value: Option<char>) -> bool {
    static LETTER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\p{L}$").expect("letter pattern"));
    value.is_some_and(|c| LETTER.is_match(c.encode_utf8(&mut [0; 4])))
}
