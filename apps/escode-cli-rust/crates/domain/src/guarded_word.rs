//! Guarded 的单词事实（TS guarded/word.ts）：去引号、标记展开，并从原文判定顶层未加引号的 glob。

use crate::guarded::CommandWord;

/// TS `literalPosixWord`：去引号与反斜杠，标记展开 / 未闭合引号为 dynamic。
pub(crate) fn literal_posix_word(text: &str, start: usize, end: usize) -> CommandWord {
    let chars: Vec<char> = text.chars().collect();
    let (mut value, mut quote, mut dynamic, mut glob) = (String::new(), None, false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if Some(c) == quote {
            quote = None;
            i += 1;
            continue;
        }
        if quote != Some('\'') && c == '\\' {
            let Some(&next) = chars.get(i + 1) else {
                dynamic = true;
                break;
            };
            if quote.is_none() || ['$', '`', '"', '\\', '\n'].contains(&next) {
                if next != '\n' {
                    value.push(next);
                }
                i += 2;
                continue;
            }
        }
        if quote.is_none() && (c == '\'' || c == '"') {
            quote = Some(c);
            i += 1;
            continue;
        }
        if quote != Some('\'') && (c == '$' || c == '`') {
            dynamic = true;
        }
        if quote.is_none() && "*?[{}~".contains(c) {
            dynamic = true;
        }
        if quote.is_none() && "*?[".contains(c) {
            glob = true;
        }
        value.push(c);
        i += 1;
    }
    CommandWord {
        value,
        dynamic: dynamic || quote.is_some(),
        has_unquoted_glob: glob,
        start,
        end,
    }
}

/// TS `commandBasename`。
pub(crate) fn command_basename(word: &CommandWord, windows: bool) -> Option<String> {
    if word.dynamic {
        return None;
    }
    let name = word
        .value
        .rsplit(|c| c == '/' || (windows && c == '\\'))
        .next()
        .unwrap_or_default();
    Some(if windows {
        let lower = name.to_lowercase();
        lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
    } else {
        name.to_owned()
    })
}


/// 由 bash 词的原文构造单词事实：dynamic / value 按 TS `literalPosixWord`；glob 只取顶层未加引号的
/// 字面片段（TS 对带 parts 的词只看 Literal 部件），展开（`$(…)`、`${…}`、反引号、`$'…'`）与引号内
/// 的 `*?[` 不算删除目标的词面证据。`start`/`end` 已是 UTF-16 偏移。
pub(crate) fn bash_word(raw: &str, start: usize, end: usize) -> CommandWord {
    let mut word = literal_posix_word(raw, start, end);
    word.has_unquoted_glob = top_level_glob(raw);
    word
}

fn top_level_glob(raw: &str) -> bool {
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '\'' => i = skip_until(&chars, i + 1, '\''),
            '"' => i = skip_double(&chars, i + 1),
            '`' => i = skip_until(&chars, i + 1, '`'),
            '$' => match chars.get(i + 1) {
                Some('(') => i = skip_nested(&chars, i + 2, '(', ')'),
                Some('{') => i = skip_nested(&chars, i + 2, '{', '}'),
                Some('\'') => i = skip_until(&chars, i + 2, '\''),
                Some('"') => i = skip_double(&chars, i + 2),
                _ => i += 1,
            },
            '*' | '?' | '[' => return true,
            _ => i += 1,
        }
    }
    false
}

fn skip_until(chars: &[char], mut i: usize, close: char) -> usize {
    while i < chars.len() && chars[i] != close {
        i += 1;
    }
    i + 1
}

fn skip_double(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i] != '"' {
        i += if chars[i] == '\\' { 2 } else { 1 };
    }
    i + 1
}

fn skip_nested(chars: &[char], mut i: usize, open: char, close: char) -> usize {
    let mut depth = 1;
    while i < chars.len() {
        if chars[i] == open {
            depth += 1;
        } else if chars[i] == close {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        i += 1;
    }
    i
}
