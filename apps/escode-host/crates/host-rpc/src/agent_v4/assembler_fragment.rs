//! fragment 物理帧的装配分支（TS wire-assembler accept 的 fragment 路径：预算/占位/齐片重组）。

use super::*;

impl TopicWireFrameAssembler {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn accept_fragment(
        &mut self,
        wire: &Value,
        key: &str,
        delivery: Delivery,
        topic: &str,
        subscription_id: &str,
        now_ms: u64,
        mut events: Vec<AssemblyEvent>,
    ) -> Vec<AssemblyEvent> {
        const KEYS: [&str; 12] = [
            "wireVersion",
            "kind",
            "deliveryKind",
            "logicalFrameId",
            "logicalFrameOrdinal",
            "topic",
            "subscriptionId",
            "fragmentIndex",
            "fragmentCount",
            "logicalBytes",
            "checksum",
            "dataBase64",
        ];
        let valid_inner = Self::keys_are(wire, &KEYS)
            && wire["fragmentIndex"].is_i64()
            && wire["fragmentCount"].is_i64()
            && wire["logicalBytes"].is_i64()
            && wire["dataBase64"].is_string()
            && wire.get("checksum").is_some_and(|checksum| {
                checksum.is_object()
                    && Self::keys_are(checksum, &["algorithm", "value"])
                    && checksum["algorithm"].is_string()
                    && checksum["value"].is_string()
            });
        if !valid_inner {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyMetadataMismatch"));
            return events;
        }
        if envelope_bytes(wire) > MAX_FRAME_BYTES {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameEnvelopeTooLarge"));
            return events;
        }
        let fragment_count = wire["fragmentCount"].as_i64().unwrap_or_default();
        let fragment_index = wire["fragmentIndex"].as_i64().unwrap_or_default();
        let logical_bytes = wire["logicalBytes"].as_i64().unwrap_or_default();
        let checksum = wire["checksum"]["value"].as_str().unwrap_or_default();
        if fragment_count > MAX_FRAGMENTS {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameFragmentCountExceeded"));
            return events;
        }
        if logical_bytes > MAX_ASSEMBLY_BYTES as i64 {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyTooLarge"));
            return events;
        }
        let valid_shape = fragment_count >= 1
            && (0..fragment_count).contains(&fragment_index)
            && logical_bytes >= 1
            && fragment_count <= logical_bytes
            && wire["checksum"]["algorithm"] == "crc32"
            && checksum.len() == 8
            && checksum
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !valid_shape {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyMetadataMismatch"));
            return events;
        }
        let Some(decoded) = decode_wire_base64(wire["dataBase64"].as_str().unwrap_or_default())
        else {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyInvalidBase64"));
            return events;
        };
        let fragment_len = decoded.len();
        if self.assemblies.contains_key(key) {
            let mismatch = self.assemblies.get(key).is_some_and(|assembly| {
                assembly.fragment_count != fragment_count as usize
                    || assembly.delivery != delivery
                    || assembly.logical_bytes != logical_bytes as usize
                    || assembly.checksum != checksum
            });
            if mismatch {
                self.release_and_settle_wire(key, wire);
                events.push(self.fault_wire(wire, "proto.frameAssemblyMetadataMismatch"));
                return events;
            }
        } else {
            if self.assemblies.len() >= MAX_CONCURRENT_ASSEMBLIES {
                self.settle_wire(key, wire);
                events.push(self.fault_wire(wire, "proto.frameAssemblyConcurrentLimit"));
                return events;
            }
            if self.staged_decoded_bytes + fragment_len > MAX_STAGED_DECODED_BYTES {
                self.settle_wire(key, wire);
                events.push(self.fault_wire(wire, "proto.frameAssemblyBudgetExceeded"));
                return events;
            }
            self.assemblies.insert(
                key.to_owned(),
                Assembly {
                    delivery,
                    id: wire["logicalFrameId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    ordinal: ordinal_of(wire).unwrap_or_default(),
                    topic: topic.to_owned(),
                    subscription_id: subscription_id.to_owned(),
                    fragment_count: fragment_count as usize,
                    logical_bytes: logical_bytes as usize,
                    checksum: checksum.to_owned(),
                    fragments: (0..fragment_count).map(|_| None).collect(),
                    received: 0,
                    decoded_bytes: 0,
                    first_seen_ms: now_ms,
                },
            );
        }
        let duplicate = self
            .assemblies
            .get(key)
            .and_then(|assembly| assembly.fragments[fragment_index as usize].clone());
        if let Some(previous) = duplicate {
            if previous != decoded {
                let (id, ordinal, current_delivery) = self.meta_of(key);
                self.release(key);
                self.settle(key, &id, ordinal);
                events.push(self.fault(
                    Some(current_delivery),
                    topic,
                    subscription_id,
                    &id,
                    ordinal,
                    "proto.frameAssemblyFragmentConflict",
                ));
            }
            return events;
        }
        if self.staged_decoded_bytes + fragment_len > MAX_STAGED_DECODED_BYTES {
            let (id, ordinal, current_delivery) = self.meta_of(key);
            self.release(key);
            self.settle(key, &id, ordinal);
            events.push(self.fault(
                Some(current_delivery),
                topic,
                subscription_id,
                &id,
                ordinal,
                "proto.frameAssemblyBudgetExceeded",
            ));
            return events;
        }
        let overflows = self
            .assemblies
            .get(key)
            .is_some_and(|assembly| assembly.decoded_bytes + fragment_len > assembly.logical_bytes);
        if overflows {
            let (id, ordinal, current_delivery) = self.meta_of(key);
            self.release(key);
            self.settle(key, &id, ordinal);
            events.push(self.fault(
                Some(current_delivery),
                topic,
                subscription_id,
                &id,
                ordinal,
                "proto.frameAssemblyLengthMismatch",
            ));
            return events;
        }
        let complete = {
            let assembly = self.assemblies.get_mut(key).unwrap();
            assembly.fragments[fragment_index as usize] = Some(decoded);
            assembly.received += 1;
            assembly.decoded_bytes += fragment_len;
            self.staged_decoded_bytes += fragment_len;
            assembly.received == assembly.fragment_count
        };
        if !complete {
            return events;
        }
        let (id, ordinal, assembly_delivery, logical_bytes, checksum, fragments, decoded_bytes) = {
            let assembly = self.assemblies.get(key).unwrap();
            (
                assembly.id.clone(),
                assembly.ordinal,
                assembly.delivery,
                assembly.logical_bytes,
                assembly.checksum.clone(),
                assembly.fragments.clone(),
                assembly.decoded_bytes,
            )
        };
        self.release(key);
        // 只捕获本地量的 fault 构造（避免与 self.settle 的可变借用冲突）。
        let fault_event = |reason: &'static str| {
            AssemblyEvent::Fault(AssemblyFault {
                delivery: Some(assembly_delivery),
                reason,
                logical_frame_id: id.clone(),
                logical_frame_ordinal: ordinal,
                topic: topic.to_owned(),
                subscription_id: subscription_id.to_owned(),
            })
        };
        let frame_check = self.frame_check;
        let mut settle_fault = |reason: &'static str| {
            self.settle(key, &id, ordinal);
            events.push(fault_event(reason));
        };
        if decoded_bytes != logical_bytes {
            settle_fault("proto.frameAssemblyLengthMismatch");
            return events;
        }
        let mut logical = Vec::with_capacity(logical_bytes);
        for fragment in fragments {
            match fragment {
                Some(fragment) => logical.extend_from_slice(&fragment),
                None => {
                    settle_fault("proto.frameAssemblyLengthMismatch");
                    return events;
                }
            }
        }
        if crc32_hex(&logical) != checksum {
            settle_fault("proto.frameAssemblyChecksumMismatch");
            return events;
        }
        let Ok(text) = std::str::from_utf8(&logical) else {
            settle_fault("proto.frameAssemblyInvalidUtf8");
            return events;
        };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            settle_fault("proto.frameAssemblyInvalidJson");
            return events;
        };
        if !frame_matches_envelope(&value, topic, subscription_id) {
            settle_fault("proto.frameAssemblyMetadataMismatch");
            return events;
        }
        if !frame_check(&value) {
            settle_fault(INVALID_PAYLOAD);
            return events;
        }
        self.settle(key, &id, ordinal);
        events.push(AssemblyEvent::Complete {
            delivery: assembly_delivery,
            frame: value,
        });
        events
    }
}
