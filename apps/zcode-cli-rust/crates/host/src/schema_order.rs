//! MCP 工具 schema 的保序文本登记表（docs/specs/rust-tool-schema-order.md）：按排序文本内容寻址，
//! 工具层在 `tools/list` 后登记，模型请求编码时查询。只存 schema 文本，重复登记幂等。
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

/// 远超单进程可能的不同 schema 数；超出时整表清空而不是无界增长（查不到只会回落为排序输出）。
const LIMIT: usize = 20_000;

fn table() -> &'static Mutex<HashMap<String, String>> {
    static TABLE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

pub fn remember(sorted: String, ordered: String) {
    let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
    if table.len() >= LIMIT && !table.contains_key(&sorted) {
        table.clear();
    }
    table.insert(sorted, ordered);
}

pub fn lookup(sorted: &str) -> Option<String> {
    table().lock().unwrap_or_else(|e| e.into_inner()).get(sorted).cloned()
}
