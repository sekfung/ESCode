//! 会话已发布增量的有界内存日志，供带 base 的订阅续传（docs/specs/rust-resume-replay.md）。
use serde_json::Value;
use std::collections::VecDeque;

/// 每会话保留的已发布增量上限（按序列化字节计）。
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Default, Debug, Clone)]
pub struct DeltaLog {
    /// (该批首个增量之前的 seq, 该批增量, 序列化字节数)。seq 与增量一一对应。
    batches: VecDeque<(u64, Vec<Value>, usize)>,
    bytes: usize,
}

impl DeltaLog {
    /// 记录一批从 `from` 开始发布的增量；超出字节预算时丢弃最旧批次。
    pub fn push(&mut self, from: u64, deltas: &[Value]) {
        let size = serde_json::to_vec(deltas).map_or(0, |b| b.len());
        // 不连续（例如冷启动后 seq 已前进）时整体重来，避免拼出伪造的连续区间。
        if self.end().is_some_and(|end| end != from) {
            self.clear();
        }
        self.batches.push_back((from, deltas.to_vec(), size));
        self.bytes += size;
        while self.bytes > MAX_BYTES && self.batches.len() > 1 {
            if let Some((_, _, dropped)) = self.batches.pop_front() {
                self.bytes -= dropped;
            }
        }
        if self.bytes > MAX_BYTES {
            self.clear();
        }
    }

    pub fn clear(&mut self) {
        self.batches.clear();
        self.bytes = 0;
    }

    fn end(&self) -> Option<u64> {
        self.batches
            .back()
            .map(|(from, deltas, _)| from + deltas.len() as u64)
    }

    /// `base` 之后直到 `current` 的全部增量；base 不在保留窗口内返回 None（调用方回落 snapshot）。
    pub fn replay(&self, base: u64, current: u64) -> Option<Vec<Value>> {
        if base == current {
            return Some(vec![]);
        }
        let start = self.batches.front()?.0;
        if base < start || base > current || self.end()? != current {
            return None;
        }
        let mut out = vec![];
        for (from, deltas, _) in &self.batches {
            let end = from + deltas.len() as u64;
            if end <= base {
                continue;
            }
            let skip = base.saturating_sub(*from) as usize;
            out.extend(deltas[skip..].iter().cloned());
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ops(n: usize, tag: &str) -> Vec<Value> {
        (0..n).map(|i| json!({"op":tag,"i":i})).collect()
    }

    #[test]
    fn replays_from_inside_a_batch() {
        let mut log = DeltaLog::default();
        log.push(0, &ops(2, "a"));
        log.push(2, &ops(3, "b"));
        let replay = log.replay(3, 5).unwrap();
        assert_eq!(replay, ops(3, "b")[1..].to_vec());
        assert_eq!(log.replay(0, 5).unwrap().len(), 5);
        assert_eq!(log.replay(5, 5).unwrap(), Vec::<Value>::new());
    }

    #[test]
    fn falls_back_outside_the_window_or_after_a_gap() {
        let mut log = DeltaLog::default();
        log.push(10, &ops(2, "a"));
        assert!(log.replay(9, 12).is_none(), "before the window");
        assert!(log.replay(13, 12).is_none(), "ahead of current");
        log.push(20, &ops(1, "b"));
        assert!(log.replay(10, 21).is_none(), "gap resets the window");
        assert_eq!(log.replay(20, 21).unwrap().len(), 1);
        assert!(
            DeltaLog::default().replay(1, 3).is_none(),
            "empty after a cold start"
        );
    }

    #[test]
    fn drops_oldest_batches_over_budget() {
        let mut log = DeltaLog::default();
        let big = vec![json!({"op":"x","text":"y".repeat(600 * 1024)})];
        log.push(0, &big);
        log.push(1, &big);
        assert!(log.replay(0, 2).is_none());
        assert_eq!(log.replay(1, 2).unwrap().len(), 1);
    }
}
