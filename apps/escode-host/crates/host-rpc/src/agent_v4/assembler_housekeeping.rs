//! 装配器的记账操作（TS `expire` / `abort` / `clear` / `nextExpiryAt`）。

use super::*;

impl TopicWireFrameAssembler {
    /// TS `expire`：超时装配按首见时间淘汰。
    /// （先物化过期清单再释放：`release` 需要 `&mut self`，与遍历 `assemblies` 冲突。）
    #[allow(clippy::needless_collect)]
    pub(crate) fn expire(&mut self, now_ms: u64) -> Vec<AssemblyEvent> {
        let expired: Vec<(String, String, i64, Delivery, String, String)> = self
            .assemblies
            .values()
            .filter(|assembly| now_ms.saturating_sub(assembly.first_seen_ms) >= ASSEMBLY_TIMEOUT_MS)
            .map(|assembly| {
                (
                    route_key(&assembly.topic, &assembly.subscription_id),
                    assembly.id.clone(),
                    assembly.ordinal,
                    assembly.delivery,
                    assembly.topic.clone(),
                    assembly.subscription_id.clone(),
                )
            })
            .collect();
        expired
            .into_iter()
            .map(|(key, id, ordinal, delivery, topic, subscription_id)| {
                self.release(&key);
                self.settle(&key, &id, ordinal);
                self.fault(
                    Some(delivery),
                    &topic,
                    &subscription_id,
                    &id,
                    ordinal,
                    "proto.frameAssemblyTimedOut",
                )
            })
            .collect()
    }

    /// TS `abort`：fault 后释放在飞字节，保留 tombstone 防旧 replay 复活。
    pub(crate) fn abort(&mut self, topic: &str, subscription_id: &str) {
        let key = route_key(topic, subscription_id);
        if let Some(assembly) = self.assemblies.remove(&key) {
            self.staged_decoded_bytes -= assembly.decoded_bytes;
            self.settle(&key, &assembly.id, assembly.ordinal);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.assemblies.clear();
        self.settled.clear();
        self.staged_decoded_bytes = 0;
    }

    pub(crate) fn next_expiry_at(&self) -> Option<u64> {
        self.assemblies
            .values()
            .map(|assembly| assembly.first_seen_ms + ASSEMBLY_TIMEOUT_MS)
            .min()
    }
}
