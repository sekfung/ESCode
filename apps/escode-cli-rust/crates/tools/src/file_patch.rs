//! Write/Edit 与文件改动视图共用的结构化补丁（从 tool_files.rs 拆出以守住模块体量）。
use serde_json::{Value, json};

pub(super) fn patch(old: &str, new: &str) -> (Value, usize, usize) {
    if old == new {
        return (json!([]), 0, 0);
    }
    let a: Vec<_> = old.lines().collect();
    let b: Vec<_> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let removed = &a[prefix..a.len() - suffix];
    let added = &b[prefix..b.len() - suffix];
    let lines: Vec<_> = removed
        .iter()
        .map(|s| format!("-{s}"))
        .chain(added.iter().map(|s| format!("+{s}")))
        .collect();
    (
        json!([{"oldStart":prefix+1,"oldLines":removed.len(),"newStart":prefix+1,"newLines":added.len(),"lines":lines}]),
        added.len(),
        removed.len(),
    )
}
