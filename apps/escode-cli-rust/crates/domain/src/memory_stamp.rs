//! 记忆文件写入时补写来源会话（TS `stampMemoryOriginSessionId`）：frontmatter 的 `metadata` 缺少
//! `originSessionId` 时，首项插入 `node_type: memory` 并追加 `originSessionId`。
//! TS 以 `yaml` 文档整体重新序列化；这里按行插入并保留其余行原样，二者在常见形态（plain/引号标量、
//! 两空格块映射、单行 flow 映射）上一致；`yaml` 会折行的超长标量等少见形态可能存在格式差异（语义相同）。
use regex::Regex;
use std::sync::LazyLock;

static FRONTMATTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\x{feff}?---(?:\r\n|\n))([\s\S]*?)((?:\r\n|\n)---)(?:\r\n|\n|$)").unwrap()
});

pub fn stamp_origin(
    content: &str,
    root: &str,
    file_path: &str,
    session: &str,
    cwd: &str,
) -> String {
    if !file_path.ends_with(".md")
        || super::memory_policy::contained_segments(root, file_path, cwd).is_none()
    {
        return content.into();
    }
    let Some(captures) = FRONTMATTER.captures(content) else {
        return content.into();
    };
    let (opening, frontmatter, closing) = (&captures[1], &captures[2], &captures[3]);
    if !super::memory_yaml::is_valid(frontmatter) {
        return content.into();
    }
    let ending = if opening.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let lines: Vec<&str> = frontmatter
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let Some(stamped) = stamp_lines(&lines, session) else {
        return content.into();
    };
    let end = captures.get(3).unwrap().end();
    format!(
        "{opening}{}{closing}{}",
        stamped.join(ending),
        &content[end..]
    )
}

fn key_of(line: &str) -> Option<&str> {
    line.split_once(':').map(|(k, _)| k.trim())
}
fn stamp_lines(lines: &[&str], session: &str) -> Option<Vec<String>> {
    let origin = format!("originSessionId: {session}");
    let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t');
    // 缺少 metadata 时 TS 的 document.set 得到的不是 YAMLMap，原样返回。
    let index = lines
        .iter()
        .position(|l| !indented(l) && key_of(l) == Some("metadata"))?;
    let value = lines[index].split_once(':').map_or("", |(_, v)| v.trim());
    let mut out: Vec<String> = lines[..index].iter().map(|l| (*l).to_owned()).collect();
    if let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
        let mut pairs: Vec<(String, String)> = vec![];
        for pair in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once(':')?;
            pairs.push((k.trim().to_owned(), v.trim().to_owned()));
        }
        if pairs
            .iter()
            .any(|(k, v)| k == "originSessionId" && !v.is_empty())
        {
            return None;
        }
        pairs.retain(|(k, _)| k != "node_type");
        match pairs.iter_mut().find(|(k, _)| k == "originSessionId") {
            Some(existing) => existing.1 = session.into(),
            None => pairs.push(("originSessionId".into(), session.into())),
        }
        pairs.insert(0, ("node_type".into(), "memory".into()));
        let body = pairs
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("metadata: {{ {body} }}"));
        out.extend(lines[index + 1..].iter().map(|l| (*l).to_owned()));
        return Some(out);
    }
    if !value.is_empty() {
        // 非 mapping 的 metadata 不修复（TS 同样原样返回）。
        return None;
    }
    let children_end = (index + 1..lines.len())
        .find(|i| !lines[*i].trim().is_empty() && !indented(lines[*i]))
        .unwrap_or(lines.len());
    let children = &lines[index + 1..children_end];
    let depth = |l: &str| l.len() - l.trim_start().len();
    let indent = children
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| depth(l))
        .min()?;
    // 已有非空 originSessionId 时不改写。
    if children.iter().any(|l| {
        depth(l) == indent
            && key_of(l) == Some("originSessionId")
            && l.split_once(':').is_some_and(|(_, v)| !v.trim().is_empty())
    }) {
        return None;
    }
    out.push("metadata:".into());
    out.push("  node_type: memory".into());
    // node_type 移到首项；已有的空 originSessionId 原位赋值（YAMLMap.set），否则追加到末尾。
    let mut skip_nested = false;
    let mut placed = false;
    for line in children {
        if depth(line) == indent && !line.trim().is_empty() {
            skip_nested = false;
            match key_of(line) {
                Some("node_type") => {
                    skip_nested = true;
                    continue;
                }
                Some("originSessionId") => {
                    out.push(format!("  {origin}"));
                    placed = true;
                    skip_nested = true;
                    continue;
                }
                _ => {}
            }
        }
        if skip_nested {
            continue;
        }
        // `yaml` 以两空格缩进重新输出映射子项。
        out.push(format!(
            "  {}",
            line.get(indent..).unwrap_or(line.trim_start())
        ));
    }
    if !placed {
        out.push(format!("  {origin}"));
    }
    out.extend(lines[children_end..].iter().map(|l| (*l).to_owned()));
    Some(out)
}
