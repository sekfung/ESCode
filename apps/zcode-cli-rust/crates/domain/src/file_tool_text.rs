//! Read/Write/Edit 的模型可见文案（docs/specs/rust-file-tool-results.md），逐字对齐 TS
//! `core/src/tool/handlers/{write,edit,read}.ts`。纯函数。

pub const NOT_READ: &str = "File has not been read yet. Read it first before writing to it.";
pub const STALE: &str =
    "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";
/// TS `FILE_UNCHANGED_STUB`（重复 Read 同一未变范围）。
pub const FILE_UNCHANGED: &str =
    "Wasted call \u{2014} file unchanged since your last Read. Refer to that earlier tool_result instead.";
pub const NO_CHANGE: &str = "No changes to make: old_string and new_string are exactly the same.";
pub const EXISTS_NO_OLD_STRING: &str = "Cannot create new file - file already exists.";
const FRESH: &str = " (file state is current in your context \u{2014} no need to Read it back)";

/// TS `formatWriteModelContent`（未经用户改写）。
pub fn write_success(path: &str, created: bool) -> String {
    if created {
        format!("File created successfully at: {path}{FRESH}")
    } else {
        format!("The file {path} has been updated successfully.{FRESH}")
    }
}

/// TS `formatEditModelContent`（未经用户改写）。
pub fn edit_success(path: &str, replace_all: bool) -> String {
    if replace_all {
        format!("The file {path} has been updated. All occurrences were successfully replaced.{FRESH}")
    } else {
        format!("The file {path} has been updated successfully.{FRESH}")
    }
}

/// TS `createMissing{Read,Edit}FileMessage`。
pub fn missing_file(cwd: &str, suggestion: Option<&str>) -> String {
    let mut text = format!("File does not exist. Note: your current working directory is {cwd}.");
    if let Some(name) = suggestion {
        text.push_str(&format!(" Did you mean {name}?"));
    }
    text
}

/// TS task-output（处理器失败）与 task-stop（普通错误）的未找到文案；其余工具沿用 Rust 文案。
pub fn task_not_found(tool: &str, id: &str) -> anyhow::Error {
    let message = format!("No task found with ID: {id}");
    match tool {
        "TaskOutput" => crate::tool_failure::ToolHandlerFailure(message).into(),
        "TaskStop" => anyhow::anyhow!(message),
        _ => anyhow::anyhow!("Task unavailable in this session"),
    }
}

/// TS `findSimilarFilename`：候选为同目录文件（不含目标），按名称排序；先取主名相同者，否则取距离 ≤ 3 者。
pub fn similar_filename(target: &str, names: &[String]) -> Option<String> {
    let mut entries: Vec<&String> = names.iter().filter(|n| n.as_str() != target).collect();
    // JS 默认排序按 UTF-16 码元比较。
    entries.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let stem = stem(target);
    if let Some(same) = entries.iter().find(|n| self::stem(n) == stem) {
        return Some((*same).clone());
    }
    entries.into_iter().find(|n| levenshtein(n, target) <= 3).cloned()
}

/// Node `path.basename(name, path.extname(name))`：扩展名为最后一个点起的部分，开头的点不算扩展名。
fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && !name[..index].chars().all(|c| c == '.') => &name[..index],
        _ => name,
    }
}

fn levenshtein(left: &str, right: &str) -> usize {
    let (left, right): (Vec<u16>, Vec<u16>) = (left.encode_utf16().collect(), right.encode_utf16().collect());
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (i, l) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let cost = usize::from(l != r);
            current[j + 1] = (current[j] + 1).min(previous[j + 1] + 1).min(previous[j] + cost);
        }
        previous.clone_from(&current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn suggests_same_stem_then_close_names() {
        let listed = names(&["new.txt", "existing.txt", "empty.txt"]);
        assert_eq!(similar_filename("existin.txt", &listed).as_deref(), Some("existing.txt"));
        assert_eq!(similar_filename("nope.txt", &listed).as_deref(), Some("new.txt"));
        assert_eq!(similar_filename("existing.md", &listed).as_deref(), Some("existing.txt"));
        assert_eq!(similar_filename("zzzzzzzz.rs", &listed), None);
        assert_eq!(stem(".bashrc"), ".bashrc");
        assert_eq!(stem("a.b.c"), "a.b");
    }

    #[test]
    fn success_variants_match_ts() {
        assert_eq!(write_success("a.txt", true), "File created successfully at: a.txt (file state is current in your context \u{2014} no need to Read it back)");
        assert!(write_success("a.txt", false).starts_with("The file a.txt has been updated successfully. ("));
        assert!(edit_success("a.txt", true).starts_with("The file a.txt has been updated. All occurrences were successfully replaced. ("));
        assert_eq!(missing_file("/w", Some("b.txt")), "File does not exist. Note: your current working directory is /w. Did you mean b.txt?");
    }
}
