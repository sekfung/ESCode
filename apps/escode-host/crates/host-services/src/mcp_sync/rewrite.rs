//! 远端导入时的 filesystem MCP 路径改写（TS `rewriteFilesystemMcpConfig`）。
//!
//! 只有 stdio 的 filesystem server 的 `args` 里、落在本机 home/workspace 内的绝对路径
//! 才会改写到远端对应根；其它参数原样保留。Windows 客户端同步到 Linux/WSL 时按**远端路径形态**
//! 决定拼接风格（不能用本机平台）。

use serde_json::Value;

use crate::git::paths::js_trim;

pub struct RewritePaths<'a> {
    pub local_home: &'a str,
    pub local_workspace: Option<&'a str>,
    pub remote_home: &'a str,
    pub remote_workspace: Option<&'a str>,
}

pub fn rewrite_filesystem_mcp_config(name: &str, config: &Value, paths: &RewritePaths) -> Value {
    if !is_stdio(config) || !is_filesystem(name, config) {
        return config.clone();
    }
    let Some(args) = config.get("args").and_then(Value::as_array) else {
        return config.clone();
    };
    let rewritten: Vec<Value> = args
        .iter()
        .map(|arg| match arg.as_str() {
            Some(arg) => Value::from(rewrite_path_arg(arg, paths)),
            None => arg.clone(),
        })
        .collect();
    let mut next = config.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("args".to_owned(), Value::Array(rewritten));
    }
    next
}

fn is_stdio(config: &Value) -> bool {
    let kind = config
        .get("type")
        .and_then(Value::as_str)
        .map(|value| js_trim(value).to_lowercase())
        .unwrap_or_default();
    if kind.is_empty() {
        let command = config
            .get("command")
            .and_then(Value::as_str)
            .map(js_trim)
            .unwrap_or_default();
        return !command.is_empty();
    }
    kind == "stdio"
}

fn is_filesystem(name: &str, config: &Value) -> bool {
    let normalized = js_trim(name).to_lowercase();
    if matches!(normalized.as_str(), "filesystem" | "file-system" | "fs") {
        return true;
    }
    let mut haystack = Vec::new();
    if let Some(command) = config.get("command").and_then(Value::as_str) {
        haystack.push(command.to_owned());
    }
    if let Some(args) = config.get("args").and_then(Value::as_array) {
        haystack.extend(args.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    let haystack = haystack.join(" ").to_lowercase();
    haystack.contains("@modelcontextprotocol/server-filesystem")
        || haystack.contains("mcp-server-filesystem")
}

fn rewrite_path_arg(arg: &str, paths: &RewritePaths) -> String {
    if let Some(relative) = get_relative_path_within(paths.local_workspace, arg)
        && paths
            .remote_workspace
            .is_some_and(|workspace| !js_trim(workspace).is_empty())
    {
        return join_remote_path(paths.remote_workspace.unwrap_or_default(), &relative);
    }
    if let Some(relative) = get_relative_path_within(Some(paths.local_home), arg) {
        return join_remote_path(paths.remote_home, &relative);
    }
    arg.to_owned()
}

fn get_relative_path_within(base: Option<&str>, candidate: &str) -> Option<String> {
    let base = base?.trim();
    if base.is_empty() {
        return None;
    }
    let (base, case_insensitive) = normalize_comparable(base)?;
    let (candidate, _) = normalize_comparable(candidate)?;
    if case_insensitive {
        relative_from(&base.to_lowercase(), &candidate.to_lowercase(), &candidate)
    } else {
        relative_from(&base, &candidate, &candidate)
    }
}

/// 比较用小写（Windows），切片用原文。
fn relative_from(
    base_compare: &str,
    candidate_compare: &str,
    candidate_raw: &str,
) -> Option<String> {
    if candidate_compare == base_compare {
        return Some(String::new());
    }
    let prefix = if base_compare.ends_with('/') {
        base_compare.to_owned()
    } else {
        format!("{base_compare}/")
    };
    if !candidate_compare.starts_with(&prefix) {
        return None;
    }
    // 与 TS 一样按原文切片（大小写只用于比较）；按字符数跳过，避免 Unicode 大小写折叠改变字节长度。
    Some(candidate_raw.chars().skip(prefix.chars().count()).collect())
}

/// TS `normalizeComparablePath`：只接受绝对路径；Windows 路径比较不区分大小写。
fn normalize_comparable(raw: &str) -> Option<(String, bool)> {
    let trimmed = js_trim(raw);
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let unc = trimmed.starts_with("\\\\");
    let posix = trimmed.starts_with('/');
    if !(drive || unc || posix) {
        return None;
    }
    let mut value = trimmed.replace('\\', "/");
    while value.len() > 1 && value.ends_with('/') {
        value.pop();
    }
    if value.is_empty() {
        value = "/".to_owned();
    }
    Some((value, drive || unc))
}

/// TS `joinRemotePath`：按远端根的形态选择 posix/win32 拼接。
fn join_remote_path(remote_base_path: &str, relative_path: &str) -> String {
    if relative_path.is_empty() {
        return remote_base_path.to_owned();
    }
    let segments: Vec<&str> = relative_path
        .split(['\\', '/'])
        .filter(|segment| !segment.is_empty())
        .collect();
    let base = js_trim(remote_base_path);
    let windows = {
        let bytes = base.as_bytes();
        (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
            || base.starts_with("\\\\")
    };
    if windows {
        win32_join(base, &segments)
    } else {
        posix_join(&base.replace('\\', "/"), &segments)
    }
}

fn posix_join(base: &str, segments: &[&str]) -> String {
    let absolute = base.starts_with('/');
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for segment in segments {
        for part in segment.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

fn win32_join(base: &str, segments: &[&str]) -> String {
    let normalized = base.replace('/', "\\");
    let (prefix, rest) = if let Some(stripped) = normalized.strip_prefix("\\\\") {
        let mut parts = stripped.splitn(3, '\\');
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        (
            format!("\\\\{server}\\{share}"),
            parts.next().unwrap_or_default().to_owned(),
        )
    } else if normalized.len() >= 3 && normalized.as_bytes()[1] == b':' {
        (
            normalized[..2].to_owned(),
            normalized[2..].trim_start_matches('\\').to_owned(),
        )
    } else {
        (
            String::new(),
            normalized.trim_start_matches('\\').to_owned(),
        )
    };
    let mut parts: Vec<String> = rest
        .split('\\')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    for segment in segments {
        for part in segment.split(['\\', '/']) {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other.to_owned()),
            }
        }
    }
    let joined = parts.join("\\");
    if prefix.is_empty() {
        joined
    } else if joined.is_empty() {
        prefix
    } else {
        format!("{prefix}\\{joined}")
    }
}

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
