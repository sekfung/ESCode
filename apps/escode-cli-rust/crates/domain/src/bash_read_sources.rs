//! Bash 读文件命令的识别与写入类命令标记（docs/specs/rust-bash-model-content.md），逐项对齐 TS
//! `core/src/tool/handlers/bash-read-file-sources.ts` 与 `bash-read-file-state.ts`。纯函数。
use crate::bash_parse::{Invocation, analyze};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub file_path: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub tail_lines: Option<usize>,
    pub requires_exit_zero: bool,
}

impl Source {
    fn file(file_path: String) -> Self {
        Self { file_path, start_line: None, end_line: None, tail_lines: None, requires_exit_zero: false }
    }
}

/// TS `WRITE_COMMAND_MARKERS`：格式化/自动修复类命令，可能改写已读文件。
pub fn is_write_command(command: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"--write|--fix|--in-place|--auto-correct|\brun\s+format\b|\brun\s+fix\b|\b(yarn|pnpm)\s+format\b|",
            r"\blint:file\b|\blint:fix\b|\bblack\b|\bisort\b|\bruff\s+format\b|\bcargo\s+(fmt|fix)\b|\brustfmt\b|",
            r"\bgo\s+fmt\b|\bterraform\s+fmt\b|\bdprint\s+fmt\b|\bswiftformat\b|\bphpcbf\b"
        ))
        .unwrap()
    })
    .is_match(command)
}

/// TS `collectBashReadFileSources`：整条命令只由 cat/head/tail/sed -n/单条 grep（及 echo 等无关命令）组成时，
/// 返回被读取的文件。
pub fn collect(command: &str) -> Vec<Source> {
    if command.contains(['|', '<', '>']) {
        return vec![];
    }
    let analysis = analyze(command);
    if !analysis.permission_safe() || analysis.has_redirects || analysis.commands.is_empty() {
        return vec![];
    }
    let single = analysis.commands.len() == 1;
    let mut sources = vec![];
    for part in &analysis.commands {
        let source = sed(part)
            .or_else(|| cat(part))
            .or_else(|| head_tail(part, "head", 10).map(|(file, n)| Source { start_line: Some(1), end_line: Some(n), ..Source::file(file) }))
            .or_else(|| head_tail(part, "tail", 10).map(|(file, n)| Source { tail_lines: Some(n), ..Source::file(file) }))
            .or_else(|| if single { grep(part) } else { None });
        match source {
            Some(source) => sources.push(source),
            None if !single && simple_ignored(&part.command_text) => {}
            None => return vec![],
        }
    }
    sources
}

/// TS `selectReadContent` 的结果：回填的内容片段与读取状态键里的 offset/limit。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    pub content: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// TS `selectReadContent`：超出文件行数时不回填。
pub fn select(content: &str, source: &Source) -> Option<Selected> {
    if let Some(tail) = source.tail_lines {
        let mut lines: Vec<&str> = content.split('\n').collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        if lines.is_empty() {
            return None;
        }
        let limit = tail.min(lines.len());
        let offset = lines.len() - limit + 1;
        return Some(Selected { content: lines[offset - 1..].join("\n"), offset: Some(offset), limit: Some(limit) });
    }
    let Some(start) = source.start_line else {
        return Some(Selected { content: content.to_owned(), offset: None, limit: None });
    };
    let lines: Vec<&str> = content.split('\n').collect();
    let start = start.max(1);
    let end = start.max(source.end_line.unwrap_or(start));
    if start > lines.len() {
        return None;
    }
    Some(Selected {
        content: lines[start - 1..end.min(lines.len())].join("\n"),
        offset: Some(start),
        limit: Some(end - start + 1),
    })
}

fn simple_ignored(text: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let text = text.trim_start();
    ["echo", "printf", "true", ":"].iter().any(|w| {
        text.strip_prefix(w).is_some_and(|rest| {
            let next = rest.chars().next();
            // TS ``：单词结尾后需非单词字符或结尾；`:` 是非单词字符，其后必须紧跟单词字符才构成边界。
            if *w == ":" { next.is_some_and(word) } else { next.is_none_or(|c| !word(c)) }
        })
    })
}

fn concrete(path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty() && *p != "-").map(str::to_owned)
}

fn cat(part: &Invocation) -> Option<Source> {
    if part.argv.first().map(String::as_str) != Some("cat") {
        return None;
    }
    let mut file = None;
    for arg in &part.argv[1..] {
        if arg.starts_with('-') {
            if arg != "-n" && arg != "--number" {
                return None;
            }
            continue;
        }
        if file.is_some() {
            return None;
        }
        file = Some(arg.as_str());
    }
    concrete(file).map(Source::file)
}

fn positive(value: Option<&String>) -> Option<usize> {
    value.filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit())).and_then(|v| v.parse().ok()).filter(|n| *n > 0)
}

fn head_tail(part: &Invocation, name: &str, default: usize) -> Option<(String, usize)> {
    if part.argv.first().map(String::as_str) != Some(name) {
        return None;
    }
    let (mut count, mut file) = (None, None);
    let mut args = part.argv[1..].iter();
    while let Some(arg) = args.next() {
        if arg == "-n" || arg == "--lines" {
            count = Some(positive(args.next())?);
        } else if let Some(value) = arg.strip_prefix("--lines=") {
            count = Some(positive(Some(&value.to_owned()))?);
        } else if let Some(n) = arg.strip_prefix("-n").filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            count = n.parse().ok();
        } else if let Some(n) = arg.strip_prefix('-').filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            count = n.parse().ok();
        } else if arg.starts_with('-') || file.is_some() {
            return None;
        } else {
            file = Some(arg.as_str());
        }
    }
    Some((concrete(file)?, count.unwrap_or(default)))
}

fn sed(part: &Invocation) -> Option<Source> {
    if part.argv.first().map(String::as_str) != Some("sed") {
        return None;
    }
    let (mut quiet, mut expression, mut file) = (false, None, None);
    for arg in &part.argv[1..] {
        if let Some(long) = arg.strip_prefix("--") {
            if long == "in-place" || long.starts_with("in-place=") || long == "expression" {
                return None;
            }
            quiet |= long == "quiet" || long == "silent";
        } else if arg.starts_with('-') {
            if arg.contains('i') || arg == "-e" {
                return None;
            }
            quiet |= arg.contains('n');
        } else if expression.is_none() {
            expression = Some(arg.as_str());
        } else if file.is_none() {
            file = Some(arg.as_str());
        } else {
            return None;
        }
    }
    let (file, expression) = (concrete(file)?, expression?);
    if !quiet {
        return None;
    }
    let expression = expression.strip_suffix('p')?;
    let number = |s: &str| (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse::<usize>().ok()).flatten();
    let (start, end) = match expression.split_once(',') {
        Some((a, b)) => (number(a)?, number(b)?),
        None => {
            let line = number(expression)?;
            (line, line)
        }
    };
    Some(Source { start_line: Some(start), end_line: Some(end), ..Source::file(file) })
}

fn grep(part: &Invocation) -> Option<Source> {
    if !matches!(part.argv.first().map(String::as_str), Some("grep" | "egrep" | "fgrep")) {
        return None;
    }
    const LONG: [&str; 12] = [
        "--line-number", "--ignore-case", "--word-regexp", "--line-regexp", "--extended-regexp", "--fixed-strings",
        "--basic-regexp", "--perl-regexp", "--with-filename", "--no-filename", "--color=never", "--color=auto",
    ];
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let (mut pattern, mut file) = (None, None);
    let mut args = part.argv[1..].iter();
    while let Some(arg) = args.next() {
        if arg.starts_with('-') && arg != "-" {
            if matches!(arg.as_str(), "-A" | "-B" | "-C") {
                if !args.next().is_some_and(|v| digits(v)) {
                    return None;
                }
                continue;
            }
            let short_context = arg.len() > 2 && matches!(&arg[..2], "-A" | "-B" | "-C") && digits(&arg[2..]);
            let long_context = ["--after-context=", "--before-context=", "--context="]
                .iter()
                .any(|p| arg.strip_prefix(p).is_some_and(digits));
            let short_flags = arg.len() > 1 && !arg.starts_with("--") && arg[1..].chars().all(|c| "niwxEFGPHh".contains(c));
            if short_context || long_context || short_flags || LONG.contains(&arg.as_str()) {
                continue;
            }
            return None;
        }
        if pattern.is_none() {
            pattern = Some(arg.as_str());
        } else if file.is_none() {
            file = Some(arg.as_str());
        } else {
            return None;
        }
    }
    pattern?;
    let file = concrete(file)?;
    if file.contains(['*', '?', '[', '{']) {
        return None;
    }
    Some(Source { requires_exit_zero: true, ..Source::file(file) })
}

#[cfg(test)]
#[path = "bash_read_sources_tests.rs"]
mod tests;
