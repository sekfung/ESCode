//! 迁移账本比对与十六进制助手（从 `mod.rs` 拆出以守住单文件行数上限）。

use super::schema;

/// 账本差异三分类（与生成资产逐条比对）。
pub(super) fn compare_ledger(
    ledger: &[(String, String)],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for (id, checksum) in schema::EXPECTED_MIGRATIONS {
        match ledger.iter().find(|(current, _)| current == id) {
            Some((_, actual)) if actual == checksum => {}
            Some(_) => mismatched.push((*id).to_owned()),
            None => missing.push((*id).to_owned()),
        }
    }
    let unknown = ledger
        .iter()
        .filter(|(id, _)| {
            !schema::EXPECTED_MIGRATIONS
                .iter()
                .any(|(known, _)| known == id)
        })
        .map(|(id, _)| id.clone())
        .collect();
    (missing, unknown, mismatched)
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
