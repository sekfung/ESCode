//! yaml 库 `foldFlowLines` 的逐字移植：块标量与长标量按行宽折行（yaml_emit 用）。
#[allow(unused_imports)]
use super::yaml_emit::*;

use std::collections::HashSet;

/// 折叠块正文：先做 TS `blockString` 的两步改写，再交给 `foldFlowLines('block')`：
/// 先把每个换行段 k 个换成 k+1 个（折行会吃掉一个换行），再在每个换行段之后插入缩进。
/// 折行溢出（某段超宽且没有可断点）即 TS `onOverflow`，返回 None 让调用方退回字面量。
pub(super) fn fold_block(body: &str, indent: usize) -> Option<String> {
    let pad = " ".repeat(indent);
    let chars: Vec<char> = body.chars().collect();
    let mut expanded = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\n' {
            let mut run = 0usize;
            while i < chars.len() && chars[i] == '\n' {
                i += 1;
                run += 1;
            }
            for _ in 0..=run {
                expanded.push('\n');
            }
            expanded.push_str(&pad);
        } else {
            expanded.push(chars[i]);
            i += 1;
        }
    }
    let folded = fold_flow_lines(&expanded, &pad, FoldMode::Block, Some(indent));
    (!folded.bailed && !folded.overflow).then_some(folded.text)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FoldMode {
    Flow,
    Quoted,
    Block,
}

pub(super) struct Folded {
    pub(super) text: String,
    pub(super) overflow: bool,
    pub(super) bailed: bool,
}

/// TS `foldFlowLines`（yaml `dist/stringify/foldFlowLines.js`）的逐行移植：在不超过
/// `lineWidth` 的前提下，只在「前后都不是空白的单个空格」处折行，续行以 `indent` 开头。
/// `indent_at_start` 是首行已占列数（键与 `: `）。`overflow` 表示遇到超宽且不可折的段。
pub(super) fn fold_flow_lines(
    text: &str,
    indent: &str,
    mode: FoldMode,
    indent_at_start: Option<usize>,
) -> Folded {
    let chars: Vec<char> = text.chars().collect();
    let indent_len = indent.chars().count();
    let min_content = if LINE_WIDTH < MIN_CONTENT_WIDTH {
        0
    } else {
        MIN_CONTENT_WIDTH
    };
    let end_step = std::cmp::max(1 + min_content, 1 + LINE_WIDTH.saturating_sub(indent_len));
    let unchanged = |overflow: bool, bailed: bool| Folded {
        text: text.to_owned(),
        overflow,
        bailed,
    };
    if chars.len() <= end_step {
        return unchanged(false, false);
    }
    let at = |i: i64| -> Option<char> {
        if i < 0 {
            None
        } else {
            chars.get(i as usize).copied()
        }
    };
    let mut folds: Vec<usize> = Vec::new();
    let mut escaped_folds: HashSet<usize> = HashSet::new();
    let mut end: i64 = LINE_WIDTH as i64 - indent_len as i64;
    if let Some(start_col) = indent_at_start {
        if start_col as i64 > LINE_WIDTH as i64 - std::cmp::max(2, min_content) as i64 {
            folds.push(0);
        } else {
            end = LINE_WIDTH as i64 - start_col as i64;
        }
    }
    let mut split: Option<usize> = None;
    let mut prev: Option<char> = None;
    let mut overflow = false;
    let (mut esc_start, mut esc_end): (i64, i64) = (-1, -1);
    let mut i: i64 = -1;
    if mode == FoldMode::Block {
        i = consume_more_indented_lines(&chars, i, indent_len);
        if i != -1 {
            end = i + end_step as i64;
        }
    }
    loop {
        i += 1;
        let Some(ch) = at(i) else { break };
        if mode == FoldMode::Quoted && ch == '\\' {
            esc_start = i;
            i += match at(i + 1) {
                Some('x') => 3,
                Some('u') => 5,
                Some('U') => 9,
                _ => 1,
            };
            esc_end = i;
        }
        if ch == '\n' {
            if mode == FoldMode::Block {
                i = consume_more_indented_lines(&chars, i, indent_len);
            }
            end = i + indent_len as i64 + end_step as i64;
            split = None;
        } else {
            if ch == ' '
                && matches!(prev, Some(p) if p != ' ' && p != '\n' && p != '\t')
                && !matches!(at(i + 1), None | Some(' ') | Some('\n') | Some('\t'))
            {
                split = Some(i as usize);
            }
            if i >= end {
                if let Some(point) = split {
                    folds.push(point);
                    end = point as i64 + end_step as i64;
                    split = None;
                } else if mode == FoldMode::Quoted {
                    // 没有可断点：吃掉后面的空白，在 `i - 2` 处硬折（TS 的 `escapedFolds` 分支）。
                    while matches!(prev, Some(' ') | Some('\t')) {
                        prev = Some(ch);
                        i += 1;
                        overflow = true;
                    }
                    let point = if i > esc_end + 1 {
                        i - 2
                    } else {
                        esc_start - 1
                    };
                    if point < 0 || escaped_folds.contains(&(point as usize)) {
                        return unchanged(overflow, true);
                    }
                    folds.push(point as usize);
                    escaped_folds.insert(point as usize);
                    end = point + end_step as i64;
                    split = None;
                } else {
                    overflow = true;
                }
            }
        }
        prev = Some(ch);
    }
    if folds.is_empty() {
        return unchanged(overflow, false);
    }
    let slice = |from: usize, to: usize| -> String {
        chars[from.min(chars.len())..to.min(chars.len())]
            .iter()
            .collect()
    };
    let mut res = slice(0, folds[0]);
    for (index, &point) in folds.iter().enumerate() {
        let to = folds.get(index + 1).copied().unwrap_or(chars.len());
        if point == 0 {
            res = format!("\n{indent}{}", slice(0, to));
        } else {
            if mode == FoldMode::Quoted && escaped_folds.contains(&point) {
                res.push(chars[point]);
                res.push('\\');
            }
            res.push('\n');
            res.push_str(indent);
            res.push_str(&slice(point + 1, to));
        }
    }
    Folded {
        text: res,
        overflow,
        bailed: false,
    }
}

/// TS `consumeMoreIndentedLines`：`i + 1` 视为行首，跳过 more-indented（以空白开头）的整行，
/// 返回最后一个换行的下标。本模块只在折叠块里用到（语料没有 more-indented 行的折叠用例）。
pub(super) fn consume_more_indented_lines(chars: &[char], mut i: i64, indent: usize) -> i64 {
    let at = |i: i64| -> Option<char> {
        if i < 0 {
            None
        } else {
            chars.get(i as usize).copied()
        }
    };
    let mut end = i;
    let mut start = i + 1;
    let mut ch = at(start);
    while matches!(ch, Some(' ') | Some('\t')) {
        if i < start + indent as i64 {
            i += 1;
            ch = at(i);
        } else {
            loop {
                i += 1;
                ch = at(i);
                if !matches!(ch, Some(c) if c != '\n') {
                    break;
                }
            }
            end = i;
            start = i + 1;
            ch = at(start);
        }
    }
    end
}
