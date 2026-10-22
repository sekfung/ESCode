//! svn 路径参数、diff 拆分、未纳入版本控制文件统计与 patch 合成（TS `svn/svnPaths.ts`）。

use tokio::io::AsyncReadExt;

use super::config::{DIFF_BYTES, normalize_svn_path};

const UNVERSIONED_STAT_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRef {
    pub kind: &'static str,
    pub name: String,
    pub relative_path: String,
}

impl BranchRef {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind,
            "name": self.name,
            "relativePath": self.relative_path,
        })
    }
}

/// 文件名本身带 `@` 时追加一个 `@`，避免被 svn 解析成 peg revision。
pub fn local_target(path: &str) -> String {
    if path.contains('@') {
        format!("{path}@")
    } else {
        path.to_owned()
    }
}

fn is_absolute_path(path: &str) -> bool {
    // 与 Node `path.isAbsolute` 一致：盘符只在 Windows 上算绝对路径。
    path.starts_with('/') || (cfg!(windows) && path.get(1..2) == Some(":"))
}

/// 绝对或相对路径 → 相对 workspace（`/` 分隔）。
pub fn to_workspace_relative(workspace_path: &str, absolute_or_relative: &str) -> String {
    let workspace = normalize_svn_path(workspace_path);
    let workspace = workspace.trim_end_matches('/');
    let path = normalize_svn_path(absolute_or_relative);
    if is_absolute_path(&path) {
        let prefix = format!("{workspace}/");
        if let Some(rest) = strip_prefix_compat(&path, &prefix) {
            return if rest.is_empty() { "." } else { rest }.to_owned();
        }
        if path.trim_end_matches('/') == workspace {
            return ".".to_owned();
        }
        return path;
    }
    let relative = path.strip_prefix("./").unwrap_or(&path);
    if relative.is_empty() {
        ".".to_owned()
    } else {
        relative.to_owned()
    }
}

fn strip_prefix_compat<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if let Some(rest) = value.strip_prefix(prefix) {
        return Some(rest);
    }
    if cfg!(windows) {
        let lower_value = value.to_lowercase();
        let lower_prefix = prefix.to_lowercase();
        if lower_value.starts_with(&lower_prefix) {
            return Some(&value[prefix.len()..]);
        }
    }
    None
}

/// POSIX `path.relative(from, to)`（两者都是绝对路径，用于子目录 workspace 的 canSwitch 判定）。
pub fn relative_path(from: &str, to: &str) -> String {
    let split = |value: &str| -> Vec<String> {
        normalize_svn_path(value)
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_owned)
            .collect()
    };
    let (from, to) = (split(from), split(to));
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec!["..".to_owned(); from.len() - common];
    parts.extend(to[common..].iter().cloned());
    parts.join("/")
}

pub fn is_safe_relative_path(path: &str) -> bool {
    let normalized = normalize_svn_path(path);
    !normalized.is_empty()
        && !is_absolute_path(&normalized)
        && !normalized.split('/').any(|part| part == "..")
}

/// 按 `Index:` 拆分 diff；返回 (workspace 相对路径, section)。
pub fn split_diff_by_index(patch: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;
    let mut buffer: Vec<&str> = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("Index: ") {
            if let Some(previous) = current.take() {
                sections.push((previous, buffer.join("\n")));
            }
            current = Some(normalize_svn_path(path.trim()));
            buffer = vec![line];
            continue;
        }
        buffer.push(line);
    }
    if let Some(previous) = current {
        sections.push((previous, buffer.join("\n")));
    }
    sections
}

pub async fn is_directory(path: &str) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnversionedStat {
    pub additions: usize,
    pub is_binary: bool,
    pub is_directory: bool,
}

pub async fn read_unversioned_file_stat(absolute_path: &str) -> UnversionedStat {
    let Ok(metadata) = tokio::fs::metadata(absolute_path).await else {
        return UnversionedStat::default();
    };
    if metadata.is_dir() {
        return UnversionedStat {
            is_directory: true,
            ..UnversionedStat::default()
        };
    }
    if metadata.len() > UNVERSIONED_STAT_MAX_BYTES {
        return UnversionedStat::default();
    }
    match tokio::fs::read(absolute_path).await {
        Ok(content) => {
            if content.contains(&0) {
                return UnversionedStat {
                    is_binary: true,
                    ..UnversionedStat::default()
                };
            }
            UnversionedStat {
                additions: line_count(&content),
                ..UnversionedStat::default()
            }
        }
        Err(_) => UnversionedStat::default(),
    }
}

/// TS `text.split(/\r?\n/).length - (text.endsWith("\n") ? 1 : 0)`（空串为 0）。
fn line_count(content: &[u8]) -> usize {
    if content.is_empty() {
        return 0;
    }
    let newlines = content.iter().filter(|byte| **byte == b'\n').count();
    if content.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

#[derive(Clone, Debug, Default)]
pub struct UnversionedPatch {
    pub patch: Option<String>,
    pub is_binary: bool,
    pub truncated: bool,
}

/// 为未纳入版本控制的文件合成“全部新增”patch。
pub async fn build_unversioned_patch(absolute_path: &str, display_path: &str) -> UnversionedPatch {
    let Ok(file) = tokio::fs::File::open(absolute_path).await else {
        return UnversionedPatch::default();
    };
    let size = file
        .metadata()
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let mut content = Vec::new();
    if file
        .take(DIFF_BYTES as u64)
        .read_to_end(&mut content)
        .await
        .is_err()
    {
        return UnversionedPatch::default();
    }
    if content.contains(&0) {
        return UnversionedPatch {
            patch: None,
            is_binary: true,
            truncated: false,
        };
    }
    let text = String::from_utf8_lossy(&content);
    let mut lines: Vec<&str> = text.split('\n').map(strip_cr).collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut patch_lines = vec![
        format!("Index: {display_path}"),
        "===================================================================".to_owned(),
        format!("--- {display_path}\t(nonexistent)"),
        format!("+++ {display_path}\t(working copy)"),
        format!("@@ -0,0 +1,{} @@", lines.len()),
    ];
    patch_lines.extend(lines.into_iter().map(|line| format!("+{line}")));
    UnversionedPatch {
        patch: Some(patch_lines.join("\n")),
        is_binary: false,
        truncated: size > content.len() as u64,
    }
}

fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// TS `deriveSvnBranchRef`：只认标准布局，其余一律为 other。
pub fn derive_branch_ref(input: &str) -> BranchRef {
    let relative = input
        .strip_prefix('^')
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/');
    if relative == "trunk" || relative.starts_with("trunk/") {
        return BranchRef {
            kind: "trunk",
            name: "trunk".to_owned(),
            relative_path: "trunk".to_owned(),
        };
    }
    for (prefix, kind) in [("branches/", "branch"), ("tags/", "tag")] {
        if let Some(rest) = relative.strip_prefix(prefix) {
            let name = rest.split('/').next().unwrap_or_default();
            if !name.is_empty() {
                return BranchRef {
                    kind,
                    name: name.to_owned(),
                    relative_path: format!("{prefix}{name}"),
                };
            }
        }
    }
    BranchRef {
        kind: "other",
        name: if relative.is_empty() {
            "/".to_owned()
        } else {
            relative.to_owned()
        },
        relative_path: relative.to_owned(),
    }
}

/// TS `getSvnRelativeRepositoryPath`：URL 相对仓库根的路径（URL 解码）。
pub fn relative_repository_path(url: &str, repository_root: &str) -> Option<String> {
    let root = repository_root.trim_end_matches('/');
    if url != root && !url.starts_with(&format!("{root}/")) {
        return None;
    }
    let raw = url[root.len()..].trim_start_matches('/');
    Some(percent_decode(raw))
}

pub(crate) fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                output.push(byte);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// `encodeURIComponent` 的字符集。
pub fn encode_segment(segment: &str) -> String {
    let mut output = String::new();
    for byte in segment.as_bytes() {
        let ch = *byte as char;
        if byte.is_ascii_alphanumeric() || "-_.!~*'()".contains(ch) {
            output.push(ch);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}
