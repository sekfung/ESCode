//! 集合相似度候选：Jaccard 阈值下的精确前缀过滤（prefix filtering）。
//!
//! 原理：按全局「稀有优先」的确定顺序排列每个集合的词；若 J(A,B) ≥ t，则 A 的前 |A|-⌈t·|A|⌉+1 个词与
//! B 的同长前缀至少共享一个词。因此只需比较前缀有交集的对，再做精确校验，结果与两两比较完全一致。
//! 用于冲突检测（重复条目、规范句矛盾），避免条目数增大时的全量 O(n²) 集合运算。
use std::collections::{BTreeSet, HashMap};

/// 前缀长度；`t·len` 的浮点误差只会让前缀变长（更多候选），不会漏判。
fn prefix_len(len: usize, threshold: f64) -> usize {
    let required = (threshold * len as f64 - 1e-9).ceil().max(0.0) as usize;
    (len + 1).saturating_sub(required).min(len)
}

/// 精确 Jaccard：两个有序集合归并求交，并集由大小推出。
pub fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// 返回所有满足 `eligible(a, b)` 且 Jaccard ≥ `threshold` 的下标对 `(a, b)`（`a < b`，按字典序）。
/// `threshold` 必须大于 0：阈值为 0 时任意两个集合都相似，前缀过滤没有意义。
pub fn similar_pairs(
    sets: &[&BTreeSet<String>],
    threshold: f64,
    eligible: &dyn Fn(usize, usize) -> bool,
) -> Vec<(usize, usize)> {
    assert!(
        threshold > 0.0,
        "prefix filtering needs a positive threshold"
    );
    let mut frequency: HashMap<&str, usize> = HashMap::new();
    for set in sets {
        for token in set.iter() {
            *frequency.entry(token.as_str()).or_default() += 1;
        }
    }
    let mut postings: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, set) in sets.iter().enumerate() {
        let mut ordered: Vec<&str> = set.iter().map(String::as_str).collect();
        ordered.sort_by(|a, b| frequency[a].cmp(&frequency[b]).then(a.cmp(b)));
        for token in &ordered[..prefix_len(ordered.len(), threshold)] {
            postings.entry(token).or_default().push(index);
        }
    }
    let mut candidates = BTreeSet::new();
    for list in postings.values() {
        for (x, &a) in list.iter().enumerate() {
            for &b in &list[x + 1..] {
                candidates.insert((a.min(b), a.max(b)));
            }
        }
    }
    candidates
        .into_iter()
        .filter(|&(a, b)| {
            // 大小过滤：J ≥ t 要求 min/max ≥ t，先排除再做集合运算。
            let (small, large) = (
                sets[a].len().min(sets[b].len()),
                sets[a].len().max(sets[b].len()),
            );
            small as f64 >= threshold * large as f64 - 1e-9
                && eligible(a, b)
                && jaccard(sets[a], sets[b]) >= threshold
        })
        .collect()
}
