//! Markdown frontmatter（TS plugins/markdown-frontmatter.ts）：只取 name / description，支持 `>` / `|` 块标量。

use std::path::Path;

pub(super) fn read_frontmatter(path: &Path) -> (Option<String>, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_frontmatter(&content),
        Err(_) => (None, None),
    }
}

pub(super) fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    if !content.starts_with("---") {
        return (None, None);
    }
    let lines: Vec<&str> = content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    if lines[0].trim() != "---" {
        return (None, None);
    }
    let Some(end) = lines
        .iter()
        .skip(1)
        .position(|l| l.trim() == "---")
        .map(|i| i + 1)
    else {
        return (None, None);
    };
    let values = flat_scalars(&lines[1..end]);
    let get = |key: &str| {
        values
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| scalar(v))
    };
    (get("name"), get("description"))
}

pub(super) fn indented(line: &str) -> bool {
    line.starts_with(char::is_whitespace)
}

pub(super) fn flat_scalars(lines: &[&str]) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = vec![];
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() || line.trim().starts_with('#') || indented(line) {
            continue;
        }
        let Some(separator) = line.find(':').filter(|s| *s > 0) else {
            continue;
        };
        let key = line[..separator].trim().to_owned();
        let value = line[separator + 1..].trim();
        if values.iter().any(|(k, _)| *k == key) {
            continue;
        }
        let style = match value {
            ">" | ">+" | ">-" => Some(true),
            "|" | "|+" | "|-" => Some(false),
            _ => None,
        };
        let Some(folded) = style else {
            values.push((key, value.to_owned()));
            continue;
        };
        let start = index;
        while index < lines.len() && (lines[index].trim().is_empty() || indented(lines[index])) {
            index += 1;
        }
        let raw = &lines[start..index];
        let indent = raw
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let content: Vec<&str> = raw
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    ""
                } else {
                    &l[indent..]
                }
            })
            .collect();
        let value = if folded {
            let mut paragraphs: Vec<String> = vec![];
            let mut current: Vec<&str> = vec![];
            for line in content {
                if line.trim().is_empty() {
                    if !current.is_empty() {
                        paragraphs.push(current.join(" "));
                        current.clear();
                    }
                } else {
                    current.push(line.trim());
                }
            }
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
            }
            paragraphs.join("\n").trim().to_owned()
        } else {
            content.join("\n").trim().to_owned()
        };
        values.push((key, value));
    }
    values
}

pub(super) fn scalar(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let quoted = trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')));
    let value = if quoted {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_block_scalars() {
        let text = "---\nname: \"demo\"\ndescription: >\n  first line\n  second\n\n  next para\nother: x\n---\nbody";
        assert_eq!(
            parse_frontmatter(text),
            (
                Some("demo".into()),
                Some("first line second\nnext para".into())
            )
        );
        let literal = "---\r\ndescription: |\r\n  a\r\n    b\r\n---\r\n";
        assert_eq!(parse_frontmatter(literal), (None, Some("a\n  b".into())));
        assert_eq!(parse_frontmatter("no frontmatter"), (None, None));
        assert_eq!(parse_frontmatter("---\nname: ''\n---"), (None, None));
    }
}
