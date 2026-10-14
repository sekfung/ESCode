//! 回调名双向对账（rust-unified-host.md P1 #11；质量报告 §2「callback 名字集合双向对账」）：
//! 生成器导出的 `bash_policies.json` 里出现的每个 `callback` 名都必须在 Rust 声明表内，
//! 且声明表内每个名字都必须在语料里被引用。任一方向漂移都失败——替代原先未知名称走
//! `_ => true` 的危险默认而不被发现。
use crate::bash_callbacks::declared_names;
use crate::bash_safe_flag::SafeFlagKind;
use std::collections::BTreeSet;

/// 语料里的全部回调名（JSON 文本级扫描：`"callback":"<name>"`；生成器以固定键序写盘）。
fn corpus_names() -> BTreeSet<String> {
    let raw = include_str!("bash_policies.json");
    let mut names = BTreeSet::new();
    for chunk in raw.split("\"callback\":\"").skip(1) {
        let name = chunk.split('"').next().unwrap_or_default();
        if !name.is_empty() {
            names.insert(name.to_owned());
        }
    }
    names
}

#[test]
fn every_corpus_callback_has_an_implementation() {
    let declared: BTreeSet<String> = declared_names().iter().map(|n| (*n).to_owned()).collect();
    let missing: Vec<_> = corpus_names().difference(&declared).cloned().collect();
    assert!(
        missing.is_empty(),
        "语料回调在 Rust 声明表中缺失（未知名称会按危险处理，静默拒绝全部调用）：{missing:?}"
    );
}

#[test]
fn every_declared_callback_is_referenced_by_the_corpus() {
    let declared: BTreeSet<String> = declared_names().iter().map(|n| (*n).to_owned()).collect();
    let orphans: Vec<_> = declared.difference(&corpus_names()).cloned().collect();
    assert!(
        orphans.is_empty(),
        "Rust 声明表里的回调在语料中无引用（TS 侧可能已删除）：{orphans:?}"
    );
}

/// 语料里的全部 safeFlags kind 字符串（JSON 语义扫描，非文本切分）。
fn corpus_kinds() -> BTreeSet<String> {
    let root: serde_json::Value =
        serde_json::from_str(include_str!("bash_policies.json")).expect("generated policies");
    let mut kinds = BTreeSet::new();
    collect_kinds(&root, &mut kinds);
    kinds
}

fn collect_kinds(value: &serde_json::Value, kinds: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(flags) = map.get("safeFlags").and_then(serde_json::Value::as_object) {
                for kind in flags.values().filter_map(serde_json::Value::as_str) {
                    kinds.insert(kind.to_owned());
                }
            }
            for nested in map.values() {
                collect_kinds(nested, kinds);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_kinds(item, kinds);
            }
        }
        _ => {}
    }
}

/// `SafeFlagKind` 声明表（枚举变体 ↔ 语料名字的唯一来源）。
fn declared_kinds() -> BTreeSet<String> {
    [
        SafeFlagKind::None,
        SafeFlagKind::String,
        SafeFlagKind::Number,
        SafeFlagKind::Char,
        SafeFlagKind::Object,
        SafeFlagKind::Eof,
    ]
    .iter()
    .map(|kind| kind.name().to_owned())
    .collect()
}

#[test]
fn safe_flag_kinds_match_the_corpus_in_both_directions() {
    let corpus = corpus_kinds();
    assert!(!corpus.is_empty(), "语料必须含 safeFlags kind");
    assert_eq!(
        corpus,
        declared_kinds(),
        "safeFlags kind 名字集合与 Rust 枚举不一致（新增 kind 必须先类型化，加载期未知 kind 会 panic）"
    );
}

#[test]
fn safe_flag_kind_matching_follows_the_corpus_semantics() {
    assert!(SafeFlagKind::None.matches("anything"));
    assert!(SafeFlagKind::String.matches("value"));
    assert!(SafeFlagKind::Number.matches("12"));
    assert!(!SafeFlagKind::Number.matches("1x"));
    assert!(SafeFlagKind::Char.matches("c"));
    assert!(!SafeFlagKind::Char.matches("cc"));
    assert!(SafeFlagKind::Object.matches("{}"));
    assert!(!SafeFlagKind::Object.matches("{ }"));
    assert!(SafeFlagKind::Eof.matches("EOF"));
    assert!(!SafeFlagKind::Eof.matches("eof"));
}
