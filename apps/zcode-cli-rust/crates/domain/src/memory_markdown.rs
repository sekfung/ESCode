//! MEMORY.md 顶层 HTML 注释剥离（TS `stripTopLevelMarkdownHtmlComments`，基于 marked 块级 lexer）。
//! 只识别块级结构中影响结果的部分：fenced code、列表项续行、引用块与 ≥4 空格缩进代码不属于顶层；
//! 顶层以 `<!--` 开头的行开启 HTML 块，到含 `-->` 的行结束并吞掉其后的空行，与 marked 的 html token 相同。
use regex::Regex;
use std::sync::LazyLock;

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<!--[\s\S]*?-->").unwrap());
static LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([-*+]|\d{1,9}[.)])( +|$)").unwrap());

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}
fn blank(line: &str) -> bool {
    line.trim().is_empty()
}
/// fenced code 起止标记：字符与长度（≥3）。
fn fence(line: &str) -> Option<(char, usize)> {
    if indent(line) > 3 {
        return None;
    }
    let rest = line.trim_start_matches(' ');
    let marker = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let count = rest.chars().take_while(|c| *c == marker).count();
    (count >= 3).then_some((marker, count))
}

pub fn strip_top_level_html_comments(content: &str) -> String {
    if !content.contains("<!--") {
        return content.into();
    }
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut open_fence: Option<(char, usize)> = None;
    let mut list_indent: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let body = line.trim_end_matches(['\n', '\r']);
        i += 1;
        if let Some((marker, count)) = open_fence {
            out.push_str(line);
            if fence(body).is_some_and(|(m, c)| m == marker && c >= count)
                && body
                    .trim_start_matches(' ')
                    .trim_start_matches(marker)
                    .trim()
                    .is_empty()
            {
                open_fence = None;
            }
            continue;
        }
        if blank(body) {
            out.push_str(line);
            continue;
        }
        let spaces = indent(body);
        if let Some(n) = list_indent {
            if spaces >= n {
                out.push_str(line);
                continue;
            }
            list_indent = None;
        }
        if let Some(opened) = fence(body) {
            open_fence = Some(opened);
            out.push_str(line);
            continue;
        }
        let rest = &body[spaces..];
        if spaces <= 3 {
            if let Some(m) = LIST_MARKER.captures(rest) {
                let padding = m[2].len();
                let padding = if (1..=4).contains(&padding) {
                    padding
                } else {
                    1
                };
                list_indent = Some(spaces + m[1].len() + padding);
                out.push_str(line);
                continue;
            }
            if rest.starts_with('>') {
                out.push_str(line);
                continue;
            }
            if rest.starts_with("<!--") {
                let mut raw = line.to_owned();
                let mut closed = rest.contains("-->");
                while !closed && i < lines.len() {
                    raw.push_str(lines[i]);
                    closed = lines[i].contains("-->");
                    i += 1;
                }
                while i < lines.len() && blank(lines[i]) {
                    raw.push_str(lines[i]);
                    i += 1;
                }
                let without = COMMENT.replace_all(&raw, "");
                if !without.trim().is_empty() {
                    out.push_str(&without);
                }
                continue;
            }
        }
        out.push_str(line);
    }
    out
}
