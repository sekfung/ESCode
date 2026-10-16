//! `skills` 服务的辅助：提示词提及解析、激活块、目录复制/删除。

use std::path::Path;

use serde_json::Value;

use super::service::SkillsError;

type Result<T> = std::result::Result<T, SkillsError>;

pub(super) async fn remove_all(path: &Path) -> Result<()> {
    let result = match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(path).await,
        Ok(_) => tokio::fs::remove_file(path).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    result.map_err(|e| SkillsError(e.to_string()))
}

/// `fs.cp(src, dst, {recursive: true})`：符号链接按链接复制。
pub(super) fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    if meta.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(std::fs::read_link(source)?, target)?;
    } else {
        std::fs::copy(source, target)?;
    }
    Ok(())
}

/// `/\$([a-z0-9]+(?:-[a-z0-9]+)*)/g`。
pub(super) fn mentioned_names(prompt: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let chars: Vec<char> = prompt.chars().collect();
    let word = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '$' {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        let mut name = String::new();
        loop {
            let start = end;
            while end < chars.len() && word(chars[end]) {
                end += 1;
            }
            if end == start {
                break;
            }
            name.extend(&chars[start..end]);
            if end + 1 < chars.len() && chars[end] == '-' && word(chars[end + 1]) {
                name.push('-');
                end += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            index += 1;
        } else {
            names.insert(name);
            index = end;
        }
    }
    names
}

pub(super) fn xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn activated_block(skills: &[&Value]) -> String {
    let mut lines = vec!["<available_skills>".to_owned()];
    for skill in skills {
        lines.push(format!(
            "<activated_skill name=\"{}\" path=\"{}\">\n{}\n</activated_skill>",
            xml_attr(skill["name"].as_str().unwrap_or_default()),
            xml_attr(skill["path"].as_str().unwrap_or_default()),
            skill["body"].as_str().unwrap_or_default()
        ));
    }
    lines.push("</available_skills>".to_owned());
    lines.join("\n")
}
