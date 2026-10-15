//! 工作区文件索引的列式打包（TS `packages/shared/src/workspaceFileEntriesCodec.ts`）。
//! 每行 `type\t<转义 relativePath>`；`\` / TAB / LF 转义。长度语义是 JS `string.length`（UTF-16 码元）。

#[cfg(test)]
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    pub directory: bool,
    pub relative_path: String,
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

/// 仅测试用：证明「直接由 entries 建候选」与 TS 的 pack→unpack 往返等价（Host 不需要真的解包）。
/// JS `value.replace(/\\(.)/g, …)`：`.` 不匹配行终止符，所以 `\`+换行 不构成转义。
#[cfg(test)]
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('t') => {
                out.push('\t');
                chars.next();
            }
            Some('n') => {
                out.push('\n');
                chars.next();
            }
            Some(next) if !matches!(next, '\n' | '\r' | '\u{2028}' | '\u{2029}') => {
                out.push(*next);
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    out
}

pub fn pack(entries: &[IndexEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{}\t{}",
                if entry.directory { "directory" } else { "file" },
                escape(&entry.relative_path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `rootPath` → (前缀, 分隔符)：含 `\` 用 `\`，否则 `/`；已以分隔符结尾不再补。
pub fn root_prefix(root_path: &str) -> (String, char) {
    let separator = if root_path.contains('\\') { '\\' } else { '/' };
    let prefix = if root_path.ends_with('/') || root_path.ends_with('\\') {
        root_path.to_owned()
    } else {
        format!("{root_path}{separator}")
    };
    (prefix, separator)
}

/// 由 relativePath 还原 `(name, path)`（TS `unpackWorkspaceFileEntries` 的拼接规则；Windows 根下 `/` 换回 `\`）。
pub fn name_and_path(relative: &str, prefix: &str, separator: char) -> (String, String) {
    let name = relative.rsplit('/').next().unwrap_or(relative).to_owned();
    let path_part = if separator == '/' {
        relative.to_owned()
    } else {
        relative.replace('/', "\\")
    };
    (name, format!("{prefix}{path_part}"))
}

/// 解包并拼回 `{name,path,relativePath,type}`（TS `unpackWorkspaceFileEntries`）。
#[cfg(test)]
pub fn unpack(packed: &str, root_path: &str) -> Vec<Value> {
    if packed.is_empty() {
        return Vec::new();
    }
    let (prefix, separator) = root_prefix(root_path);
    packed
        .split('\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (kind, rest) = line.split_once('\t')?;
            let relative = unescape(rest);
            let (name, path) = name_and_path(&relative, &prefix, separator);
            Some(json!({
                "name": name,
                "path": path,
                "relativePath": relative,
                "type": if kind == "directory" { "directory" } else { "file" },
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_round_trips_awkward_names() {
        let names = [
            "a/b.txt",
            "tab\tname",
            "new\nline",
            "back\\slash",
            "t\\t-literal",
            "中文/😀.md",
        ];
        let entries: Vec<IndexEntry> = names
            .iter()
            .map(|name| IndexEntry {
                directory: name.ends_with("slash"),
                relative_path: (*name).to_owned(),
            })
            .collect();
        let unpacked = unpack(&pack(&entries), "/root");
        let relatives: Vec<&str> = unpacked
            .iter()
            .map(|v| v["relativePath"].as_str().unwrap())
            .collect();
        assert_eq!(relatives, names);
        assert_eq!(unpacked[0]["name"], "b.txt");
        assert_eq!(unpacked[0]["path"], "/root/a/b.txt");
        assert_eq!(unpacked[3]["type"], "directory");
    }

    #[test]
    fn windows_root_uses_backslashes() {
        let entries = [IndexEntry {
            directory: false,
            relative_path: "a/b.txt".into(),
        }];
        let unpacked = unpack(&pack(&entries), "C:\\root");
        assert_eq!(unpacked[0]["path"], "C:\\root\\a\\b.txt");
        assert_eq!(unpacked[0]["relativePath"], "a/b.txt");
    }
}
