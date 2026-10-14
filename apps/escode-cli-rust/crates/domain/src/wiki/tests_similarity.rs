//! 前缀过滤与两两比较的等价性（similarity.rs），以及冲突检测在大规模下的结果不变。
use super::similarity::{jaccard, similar_pairs};
use std::collections::BTreeSet;

/// 确定性伪随机（xorshift），避免引入随机数依赖。
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn random_sets(seed: u64, count: usize, vocabulary: u64) -> Vec<BTreeSet<String>> {
    let mut rng = Rng(seed);
    let mut sets: Vec<BTreeSet<String>> = (0..count)
        .map(|_| {
            let len = 1 + (rng.next() % 12) as usize;
            (0..len)
                .map(|_| format!("w{}", rng.next() % vocabulary))
                .collect()
        })
        .collect();
    // 混入近似重复与完全重复，确保高阈值下有命中。
    for i in 0..count / 5 {
        let mut copy = sets[i].clone();
        if i % 2 == 0 {
            copy.insert(format!("extra{i}"));
        }
        sets.push(copy);
    }
    sets
}

fn brute_force(sets: &[BTreeSet<String>], threshold: f64) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for a in 0..sets.len() {
        for b in a + 1..sets.len() {
            if jaccard(&sets[a], &sets[b]) >= threshold {
                out.push((a, b));
            }
        }
    }
    out
}

#[test]
fn prefix_filtering_matches_brute_force() {
    for (seed, vocabulary) in [(7, 15), (11, 40), (23, 200)] {
        let sets = random_sets(seed, 150, vocabulary);
        let refs: Vec<&BTreeSet<String>> = sets.iter().collect();
        for threshold in [0.3, 0.5, 0.75, 0.9, 1.0] {
            assert_eq!(
                similar_pairs(&refs, threshold, &|_, _| true),
                brute_force(&sets, threshold),
                "seed {seed} vocabulary {vocabulary} threshold {threshold}"
            );
        }
    }
}

#[test]
fn eligibility_filter_and_empty_sets() {
    let sets: Vec<BTreeSet<String>> = vec![
        ["a", "b"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        ["a", "b"]
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        BTreeSet::new(),
        BTreeSet::new(),
    ];
    let refs: Vec<&BTreeSet<String>> = sets.iter().collect();
    assert_eq!(similar_pairs(&refs, 0.9, &|_, _| true), vec![(0, 1)]);
    assert!(similar_pairs(&refs, 0.9, &|a, b| (a, b) != (0, 1)).is_empty());
}
