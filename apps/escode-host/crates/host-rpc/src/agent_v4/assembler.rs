//! V4 topic 物理帧装配器（TS `packages/shared/src/escode-protocol-v4/wire-assembler.ts`
//! `TopicWireFrameAssembler` 的 Host 侧同构）。Renderer 的 `onDynamic*Frame` 走自己的解码器，
//! Host 只有任务索引 syncer 消费 CLI 的逻辑帧，因此本模块当前只被 syncer 使用。
//!
//! 逐条对齐的规则（漏一条都会让损坏流卡死或错误放行）：
//! - 每 route（`topic\0subscriptionId`）单调 ordinal tombstone：旧 ordinal 永久静默，
//!   同 ordinal 不同 id = 冲突 fault——迟到旧片不能复活，也不能顶掉在飞的新帧；
//! - 只有 authoritative recovery 能无 fault 取代残缺旧 assembly（其余 supersede 报 fault）；
//! - 预算（TS `PROTOCOL_V4_LIMITS`）：逻辑帧 16MB、分片 1024、并发 32、暂存解码字节 32MB、
//!   30s 超时；物理信封 1MB 按 CLI NDJSON 承载计量（syncer 的帧只来自 CLI 泵，见 §6.7.4-4）；
//! - 齐片后 checksum（crc32 hex）→ UTF-8 → JSON → 信封一致 → payload 校验，全部通过才产出。

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::Value;

#[path = "assembler_complete.rs"]
mod complete;
#[path = "assembler_fragment.rs"]
mod fragment;

#[path = "assembler_housekeeping.rs"]
mod housekeeping;

#[cfg(test)]
#[path = "assembler_tests.rs"]
mod tests;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_ASSEMBLY_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAGMENTS: i64 = 1024;
const MAX_CONCURRENT_ASSEMBLIES: usize = 32;
const MAX_STAGED_DECODED_BYTES: usize = 32 * 1024 * 1024;
const ASSEMBLY_TIMEOUT_MS: u64 = 30_000;

/// TS `WIRE_FAULT_INVALID_PAYLOAD`。
pub(crate) const INVALID_PAYLOAD: &str = "proto.frameAssemblyInvalidPayload";

/// TS `TopicFrameDeliveryKind`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Initial,
    Online,
    Recovery,
}

impl Delivery {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str() {
            Some("initial") => Some(Self::Initial),
            Some("online") => Some(Self::Online),
            Some("recovery") => Some(Self::Recovery),
            _ => None,
        }
    }
}

/// TS `TopicWireAssemblyFault`。
#[derive(Clone, Debug)]
pub(crate) struct AssemblyFault {
    pub delivery: Option<Delivery>,
    pub reason: &'static str,
    pub logical_frame_id: String,
    pub logical_frame_ordinal: i64,
    pub topic: String,
    pub subscription_id: String,
}

/// TS `TopicWireAssemblyEvent`（accept/expire 的事件批，消费方见到任一 fault 即 abort）。
#[derive(Clone, Debug)]
pub(crate) enum AssemblyEvent {
    Complete { delivery: Delivery, frame: Value },
    Fault(AssemblyFault),
}

struct Assembly {
    delivery: Delivery,
    id: String,
    ordinal: i64,
    topic: String,
    subscription_id: String,
    fragment_count: usize,
    logical_bytes: usize,
    checksum: String,
    fragments: Vec<Option<Vec<u8>>>,
    received: usize,
    decoded_bytes: usize,
    first_seen_ms: u64,
}

#[derive(Clone)]
struct Settled {
    id: String,
    ordinal: i64,
}

/// 逻辑帧 payload 的最小形状校验（消费方按 topic 提供；深校验差异见 spec §6.7.4-4）。
pub(crate) type FrameCheck = fn(&Value) -> bool;

pub(crate) struct TopicWireFrameAssembler {
    assemblies: BTreeMap<String, Assembly>,
    settled: BTreeMap<String, Settled>,
    staged_decoded_bytes: usize,
    frame_check: FrameCheck,
}

fn route_key(topic: &str, subscription_id: &str) -> String {
    format!("{topic}\0{subscription_id}")
}

/// service 边界的 outer 形状（TS `topicWireFrameCandidateSchema`）：wireVersion=3、kind 二值、
/// id/topic/sub 非空字符串；其余形状交给装配器产生 typed fault（不静默丢）。
fn is_wire_candidate(wire: &Value) -> bool {
    wire["wireVersion"] == 3
        && matches!(wire["kind"].as_str(), Some("complete" | "fragment"))
        && wire["logicalFrameId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        && wire["topic"]
            .as_str()
            .is_some_and(|topic| !topic.is_empty())
        && wire["subscriptionId"]
            .as_str()
            .is_some_and(|sub| !sub.is_empty())
}

fn ordinal_of(wire: &Value) -> Option<i64> {
    // JS `Number.isSafeInteger(x) && x >= 1`。
    let ordinal = wire["logicalFrameOrdinal"].as_i64()?;
    (ordinal >= 1).then_some(ordinal)
}

/// TS `measureTopicNotificationEnvelopeBytes().maxBytes` 的 CLI 承载口径。
fn envelope_bytes(wire: &Value) -> usize {
    let envelope = serde_json::json!({"method": "v4/conversation/frame", "params": wire});
    serde_json::to_vec(&envelope).map_or(usize::MAX, |bytes| bytes.len() + 1)
}

fn crc32_hex(bytes: &[u8]) -> String {
    format!("{:08x}", crc32fast::hash(bytes))
}

/// TS `topicWireBase64Schema`：4 的倍数（≥4）、标准字母表、结尾至多 2 个 `=`。
fn decode_wire_base64(value: &str) -> Option<Vec<u8>> {
    if value.len() < 4 || !value.len().is_multiple_of(4) {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(value).ok()
}

/// TS `frameMatchesEnvelope`：逻辑帧必须回写同 topic/subId。
fn frame_matches_envelope(frame: &Value, topic: &str, subscription_id: &str) -> bool {
    frame.is_object()
        && frame["topic"].as_str() == Some(topic)
        && frame["subscriptionId"].as_str() == Some(subscription_id)
}

impl TopicWireFrameAssembler {
    pub(crate) fn new(frame_check: FrameCheck) -> Self {
        Self {
            assemblies: BTreeMap::new(),
            settled: BTreeMap::new(),
            staged_decoded_bytes: 0,
            frame_check,
        }
    }

    fn keys_are(value: &Value, allowed: &[&str]) -> bool {
        value
            .as_object()
            .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
    }

    fn fault(
        &self,
        delivery: Option<Delivery>,
        topic: &str,
        subscription_id: &str,
        id: &str,
        ordinal: i64,
        reason: &'static str,
    ) -> AssemblyEvent {
        AssemblyEvent::Fault(AssemblyFault {
            delivery,
            reason,
            logical_frame_id: id.to_owned(),
            logical_frame_ordinal: ordinal,
            topic: topic.to_owned(),
            subscription_id: subscription_id.to_owned(),
        })
    }

    /// 用 wire 自身字段构造 fault（deliveryKind 可能非法 → `None`）。
    fn fault_wire(&self, wire: &Value, reason: &'static str) -> AssemblyEvent {
        self.fault(
            Delivery::parse(&wire["deliveryKind"]),
            wire["topic"].as_str().unwrap_or_default(),
            wire["subscriptionId"].as_str().unwrap_or_default(),
            wire["logicalFrameId"].as_str().unwrap_or_default(),
            ordinal_of(wire).unwrap_or_default(),
            reason,
        )
    }

    fn settle(&mut self, key: &str, id: &str, ordinal: i64) {
        if self
            .settled
            .get(key)
            .is_some_and(|previous| previous.ordinal > ordinal)
        {
            return;
        }
        self.settled.insert(
            key.to_owned(),
            Settled {
                id: id.to_owned(),
                ordinal,
            },
        );
    }

    fn release(&mut self, key: &str) {
        if let Some(assembly) = self.assemblies.remove(key) {
            self.staged_decoded_bytes -= assembly.decoded_bytes;
        }
    }

    /// TS `releaseAndSettle(key, wire)`：同 ordinal/id 的在飞装配一并释放。
    fn release_and_settle_wire(&mut self, key: &str, wire: &Value) {
        let ordinal = ordinal_of(wire).unwrap_or_default();
        let id = wire["logicalFrameId"].as_str().unwrap_or_default();
        if self
            .assemblies
            .get(key)
            .is_some_and(|assembly| assembly.ordinal == ordinal && assembly.id == id)
        {
            self.release(key);
        }
        self.settle(key, id, ordinal);
    }

    /// 以 wire 自身 id/ordinal 写 tombstone（TS `settle(key, wire)`）。
    fn settle_wire(&mut self, key: &str, wire: &Value) {
        self.settle(
            key,
            wire["logicalFrameId"].as_str().unwrap_or_default(),
            ordinal_of(wire).unwrap_or_default(),
        );
    }

    /// 在飞装配的身份三元组（取代跨 &mut 调用持有借用）。
    fn meta_of(&self, key: &str) -> (String, i64, Delivery) {
        let assembly = self.assemblies.get(key);
        (
            assembly.map_or(String::new(), |a| a.id.clone()),
            assembly.map_or(0, |a| a.ordinal),
            assembly.map_or(Delivery::Online, |a| a.delivery),
        )
    }

    /// TS `accept`：入口先 expire，再按 tombstone/在飞装配裁决，最后按 kind 分派。
    pub(crate) fn accept(&mut self, wire: &Value, now_ms: u64) -> Vec<AssemblyEvent> {
        let mut events = self.expire(now_ms);
        if !is_wire_candidate(wire) {
            return events;
        }
        let Some(ordinal) = ordinal_of(wire) else {
            events.push(self.fault_wire(wire, "proto.frameAssemblyMetadataMismatch"));
            return events;
        };
        let topic = wire["topic"].as_str().unwrap_or_default().to_owned();
        let subscription_id = wire["subscriptionId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let id = wire["logicalFrameId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let key = route_key(&topic, &subscription_id);
        if let Some(settled) = self.settled.get(&key).cloned() {
            if ordinal < settled.ordinal {
                return events;
            }
            if ordinal == settled.ordinal {
                if settled.id != id {
                    events.push(self.fault_wire(wire, "proto.frameAssemblyOrdinalConflict"));
                }
                return events;
            }
        }
        let delivery = Delivery::parse(&wire["deliveryKind"]);
        if delivery == Some(Delivery::Recovery) {
            // 旧 online 恰在 recovery 首片到达的同次调用超时时，过滤同 route 更低 ordinal 的旧
            // fault：更高 ordinal 的权威 recovery 已取代旧 assembly，误报会关掉 route 丢帧。
            events.retain(|event| match event {
                AssemblyEvent::Fault(fault) => {
                    !(fault.topic == topic
                        && fault.subscription_id == subscription_id
                        && fault.logical_frame_ordinal < ordinal)
                }
                _ => true,
            });
        }
        if let Some(current) = self
            .assemblies
            .get(&key)
            .map(|current| (current.id.clone(), current.ordinal, current.delivery))
        {
            let (current_id, current_ordinal, current_delivery) = current;
            if ordinal < current_ordinal {
                return events;
            }
            if ordinal == current_ordinal {
                if current_id != id {
                    self.release(&key);
                    self.settle(&key, &current_id, current_ordinal);
                    events.push(self.fault_wire(wire, "proto.frameAssemblyOrdinalConflict"));
                    return events;
                }
            } else {
                self.release(&key);
                self.settle(&key, &current_id, current_ordinal);
                // same-sub recovery 本就是权威替换残缺旧帧；把旧 online 也当 fault 上报会让
                // decoder fail-close 丢掉紧随其后的完整 recovery。
                if delivery != Some(Delivery::Recovery) {
                    events.push(self.fault(
                        Some(current_delivery),
                        &topic,
                        &subscription_id,
                        &current_id,
                        current_ordinal,
                        "proto.frameAssemblySuperseded",
                    ));
                }
            }
        }
        let Some(delivery) = delivery else {
            self.release_and_settle_wire(&key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyMetadataMismatch"));
            return events;
        };
        if wire["kind"] == "complete" {
            self.accept_complete(wire, &key, delivery, &topic, &subscription_id, events)
        } else {
            self.accept_fragment(
                wire,
                &key,
                delivery,
                &topic,
                &subscription_id,
                now_ms,
                events,
            )
        }
    }
}
