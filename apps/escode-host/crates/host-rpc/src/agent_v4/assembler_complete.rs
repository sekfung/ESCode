//! complete 物理帧的装配分支（TS wire-assembler accept 的 complete 路径）。

use super::*;

impl TopicWireFrameAssembler {
    pub(super) fn accept_complete(
        &mut self,
        wire: &Value,
        key: &str,
        delivery: Delivery,
        topic: &str,
        subscription_id: &str,
        mut events: Vec<AssemblyEvent>,
    ) -> Vec<AssemblyEvent> {
        const KEYS: [&str; 8] = [
            "wireVersion",
            "kind",
            "deliveryKind",
            "logicalFrameId",
            "logicalFrameOrdinal",
            "topic",
            "subscriptionId",
            "frame",
        ];
        if !Self::keys_are(wire, &KEYS) {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyMetadataMismatch"));
            return events;
        }
        if envelope_bytes(wire) > MAX_FRAME_BYTES {
            self.release_and_settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameEnvelopeTooLarge"));
            return events;
        }
        if let Some(active) = self
            .assemblies
            .get(key)
            .map(|active| (active.id.clone(), active.ordinal, active.delivery))
        {
            let (active_id, active_ordinal, active_delivery) = active;
            self.release(key);
            self.settle(key, &active_id, active_ordinal);
            events.push(self.fault(
                Some(active_delivery),
                topic,
                subscription_id,
                &active_id,
                active_ordinal,
                "proto.frameAssemblyMetadataMismatch",
            ));
            return events;
        }
        let frame = &wire["frame"];
        let logical_bytes = frame.to_string().into_bytes().len();
        if logical_bytes > MAX_ASSEMBLY_BYTES {
            self.settle_wire(key, wire);
            events.push(self.fault_wire(wire, "proto.frameAssemblyTooLarge"));
            return events;
        }
        let verdict = if !frame_matches_envelope(frame, topic, subscription_id) {
            Some("proto.frameAssemblyMetadataMismatch")
        } else if !(self.frame_check)(frame) {
            Some(INVALID_PAYLOAD)
        } else {
            None
        };
        self.settle_wire(key, wire);
        match verdict {
            Some(reason) => events.push(self.fault_wire(wire, reason)),
            None => events.push(AssemblyEvent::Complete {
                delivery,
                frame: frame.clone(),
            }),
        }
        events
    }
}
