//! Write/Edit 与文件改动视图共用的结构化补丁（从 tool_files.rs 拆出以守住模块体量）。
//!
//! 对齐 TS `createStructuredPatch`（`core/src/tool/diff.ts`）：jsdiff 9 的 `diffLines` +
//! `structuredPatch(context: 3)`。旧实现只输出 `-`/`+` 行，缺少上下文行与
//! `\ No newline at end of file` 标记，模型可见的 structuredPatch 与 TS 不一致。
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

const CONTEXT: usize = 3;

pub(super) fn patch(old: &str, new: &str) -> (Value, usize, usize) {
    if old == new {
        return (json!([]), 0, 0);
    }
    let components = diff(&tokenize(old), &tokenize(new));
    let additions = components
        .iter()
        .filter(|c| c.added)
        .map(|c| c.lines.len())
        .sum();
    let deletions = components
        .iter()
        .filter(|c| c.removed)
        .map(|c| c.lines.len())
        .sum();
    let hunks: Vec<Value> = hunks(&components, CONTEXT)
        .into_iter()
        .map(|h| {
            json!({
                "oldStart": h.old_start, "oldLines": h.old_lines,
                "newStart": h.new_start, "newLines": h.new_lines,
                "lines": h.lines,
            })
        })
        .collect();
    (Value::Array(hunks), additions, deletions)
}

/// jsdiff `tokenize`：按行保留换行符；`\r\n` 作为整行结尾保留。
fn tokenize(value: &str) -> Vec<String> {
    value.split_inclusive('\n').map(str::to_owned).collect()
}

#[derive(Clone)]
struct Node {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<Rc<Node>>,
}

#[derive(Clone)]
struct Path {
    old_pos: i64,
    last: Option<Rc<Node>>,
}

struct Component {
    lines: Vec<String>,
    added: bool,
    removed: bool,
}

/// jsdiff `addToPath`：连续同类改动合并成一个 component。
fn add_to_path(path: &Path, added: bool, removed: bool, old_pos_inc: i64) -> Path {
    match path
        .last
        .as_ref()
        .filter(|last| last.added == added && last.removed == removed)
    {
        Some(last) => Path {
            old_pos: path.old_pos + old_pos_inc,
            last: Some(Rc::new(Node {
                count: last.count + 1,
                added,
                removed,
                previous: last.previous.clone(),
            })),
        },
        None => Path {
            old_pos: path.old_pos + old_pos_inc,
            last: Some(Rc::new(Node {
                count: 1,
                added,
                removed,
                previous: path.last.clone(),
            })),
        },
    }
}

fn extract_common(base: &mut Path, old: &[String], new: &[String], diagonal: i64) -> i64 {
    let (old_len, new_len) = (old.len() as i64, new.len() as i64);
    let mut old_pos = base.old_pos;
    let mut new_pos = old_pos - diagonal;
    let mut common = 0usize;
    while new_pos + 1 < new_len
        && old_pos + 1 < old_len
        && old[(old_pos + 1) as usize] == new[(new_pos + 1) as usize]
    {
        old_pos += 1;
        new_pos += 1;
        common += 1;
    }
    if common > 0 {
        base.last = Some(Rc::new(Node {
            count: common,
            added: false,
            removed: false,
            previous: base.last.clone(),
        }));
    }
    base.old_pos = old_pos;
    new_pos
}

/// jsdiff `Diff.diff`（Myers，默认 options）。返回按顺序排列的改动/相同组件。
fn diff(old: &[String], new: &[String]) -> Vec<Component> {
    let (old_len, new_len) = (old.len() as i64, new.len() as i64);
    let mut best: HashMap<i64, Path> = HashMap::new();
    best.insert(
        0,
        Path {
            old_pos: -1,
            last: None,
        },
    );
    let mut new_pos = extract_common(best.get_mut(&0).unwrap(), old, new, 0);
    if best[&0].old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build(best.remove(&0).and_then(|p| p.last), old, new);
    }
    let max_edit_length = old_len + new_len;
    let (mut min_diagonal, mut max_diagonal) = (i64::MIN, i64::MAX);
    let mut edit_length = 1i64;
    while edit_length <= max_edit_length {
        let mut diagonal = min_diagonal.max(-edit_length);
        let high = max_diagonal.min(edit_length);
        while diagonal <= high {
            let remove_path = best.get(&(diagonal - 1)).cloned();
            let add_path = best.get(&(diagonal + 1)).cloned();
            if remove_path.is_some() {
                best.remove(&(diagonal - 1));
            }
            let can_add = add_path.as_ref().is_some_and(|path| {
                let next = path.old_pos - diagonal;
                (0..new_len).contains(&next)
            });
            let can_remove = remove_path
                .as_ref()
                .is_some_and(|path| path.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best.remove(&diagonal);
                diagonal += 2;
                continue;
            }
            let mut base = if !can_remove
                || (can_add
                    && remove_path.as_ref().unwrap().old_pos < add_path.as_ref().unwrap().old_pos)
            {
                add_to_path(add_path.as_ref().unwrap(), true, false, 0)
            } else {
                add_to_path(remove_path.as_ref().unwrap(), false, true, 1)
            };
            new_pos = extract_common(&mut base, old, new, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build(base.last, old, new);
            }
            let old_pos = base.old_pos;
            best.insert(diagonal, base);
            if old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            diagonal += 2;
        }
        edit_length += 1;
    }
    Vec::new()
}

/// jsdiff `buildValues`：把组件链表还原成有序 token 片段。
fn build(last: Option<Rc<Node>>, old: &[String], new: &[String]) -> Vec<Component> {
    let mut nodes = Vec::new();
    let mut cursor = last;
    while let Some(node) = cursor {
        cursor = node.previous.clone();
        nodes.push(node);
    }
    nodes.reverse();
    let (mut old_pos, mut new_pos) = (0usize, 0usize);
    let mut components = Vec::with_capacity(nodes.len());
    for node in nodes {
        let lines = if node.removed {
            let slice = old[old_pos..old_pos + node.count].to_vec();
            old_pos += node.count;
            slice
        } else {
            let slice = new[new_pos..new_pos + node.count].to_vec();
            new_pos += node.count;
            if !node.added {
                old_pos += node.count;
            }
            slice
        };
        components.push(Component {
            lines,
            added: node.added,
            removed: node.removed,
        });
    }
    components
}

struct Hunk {
    old_start: i64,
    old_lines: i64,
    new_start: i64,
    new_lines: i64,
    lines: Vec<String>,
}

/// jsdiff `diffLinesResultToPatch`：按 context 分组，并补 `\ No newline at end of file`。
fn hunks(components: &[Component], context: usize) -> Vec<Hunk> {
    let mut diff: Vec<&Component> = components.iter().collect();
    let sentinel = Component {
        lines: Vec::new(),
        added: false,
        removed: false,
    };
    diff.push(&sentinel);
    let mut result = Vec::new();
    let (mut old_range_start, mut new_range_start) = (0i64, 0i64);
    let mut current: Vec<String> = Vec::new();
    let (mut old_line, mut new_line) = (1i64, 1i64);
    for (index, component) in diff.iter().enumerate() {
        let lines = &component.lines;
        if component.added || component.removed {
            if old_range_start == 0 {
                old_range_start = old_line;
                new_range_start = new_line;
                if index > 0 {
                    let previous = diff[index - 1];
                    if context > 0 {
                        let from = previous.lines.len().saturating_sub(context);
                        current = previous.lines[from..]
                            .iter()
                            .map(|line| format!(" {line}"))
                            .collect();
                    }
                    old_range_start -= current.len() as i64;
                    new_range_start -= current.len() as i64;
                }
            }
            let prefix = if component.added { '+' } else { '-' };
            current.extend(lines.iter().map(|line| format!("{prefix}{line}")));
            if component.added {
                new_line += lines.len() as i64;
            } else {
                old_line += lines.len() as i64;
            }
            continue;
        }
        if old_range_start != 0 {
            if lines.len() <= context * 2 && index < diff.len() - 2 {
                current.extend(lines.iter().map(|line| format!(" {line}")));
            } else {
                let context_size = lines.len().min(context);
                current.extend(lines[..context_size].iter().map(|line| format!(" {line}")));
                result.push(Hunk {
                    old_start: old_range_start,
                    old_lines: old_line - old_range_start + context_size as i64,
                    new_start: new_range_start,
                    new_lines: new_line - new_range_start + context_size as i64,
                    lines: finalize(std::mem::take(&mut current)),
                });
                old_range_start = 0;
                new_range_start = 0;
            }
        }
        old_line += lines.len() as i64;
        new_line += lines.len() as i64;
    }
    result
}

/// 去掉行尾换行；缺失换行时补 jsdiff 的 `\ No newline at end of file` 标记。
fn finalize(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        match line.strip_suffix('\n') {
            Some(stripped) => out.push(stripped.to_owned()),
            None => {
                out.push(line);
                out.push("\\ No newline at end of file".to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks_json(old: &str, new: &str) -> Value {
        patch(old, new).0
    }

    #[test]
    fn matches_jsdiff_single_change_with_context() {
        assert_eq!(
            hunks_json("alpha\nbeta\n", "alpha\nalpha\n"),
            json!([{"oldStart":1,"oldLines":2,"newStart":1,"newLines":2,
                "lines":[" alpha","-beta","+alpha"]}])
        );
    }

    #[test]
    fn matches_jsdiff_missing_final_newline_markers() {
        assert_eq!(
            hunks_json("a\nb", "a\nc"),
            json!([{"oldStart":1,"oldLines":2,"newStart":1,"newLines":2,
                "lines":[" a","-b","\\ No newline at end of file","+c","\\ No newline at end of file"]}])
        );
    }

    #[test]
    fn matches_jsdiff_multiple_hunks_and_merging() {
        assert_eq!(
            hunks_json(
                "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n",
                "l1\nX\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nY\nl11\nl12\n"
            ),
            json!([
                {"oldStart":1,"oldLines":5,"newStart":1,"newLines":5,
                    "lines":[" l1","-l2","+X"," l3"," l4"," l5"]},
                {"oldStart":7,"oldLines":6,"newStart":7,"newLines":6,
                    "lines":[" l7"," l8"," l9","-l10","+Y"," l11"," l12"]}
            ])
        );
        assert_eq!(
            hunks_json(
                "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n",
                "l1\nX\nl3\nl4\nl5\nl6\nl7\nY\nl9\nl10\n"
            ),
            json!([{"oldStart":1,"oldLines":10,"newStart":1,"newLines":10,
                "lines":[" l1","-l2","+X"," l3"," l4"," l5"," l6"," l7","-l8","+Y"," l9"," l10"]}])
        );
    }

    #[test]
    fn matches_jsdiff_insertion_and_full_deletion() {
        assert_eq!(
            hunks_json("a\nb\n", "z\na\nb\n"),
            json!([{"oldStart":1,"oldLines":2,"newStart":1,"newLines":3,
                "lines":["+z"," a"," b"]}])
        );
        assert_eq!(
            hunks_json("a\nb\n", ""),
            json!([{"oldStart":1,"oldLines":2,"newStart":1,"newLines":0,
                "lines":["-a","-b"]}])
        );
        assert_eq!(hunks_json("same\n", "same\n"), json!([]));
    }

    #[test]
    fn counts_added_and_removed_lines() {
        assert_eq!(
            patch("alpha\nbeta\n", "alpha\nalpha\n"),
            (hunks_json("alpha\nbeta\n", "alpha\nalpha\n"), 1, 1)
        );
    }
}
