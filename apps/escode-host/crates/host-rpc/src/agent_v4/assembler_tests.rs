//! 装配器的语义边界（对齐 TS wire-assembler 的每条规则；行号见该文件注释）。
use super::*;
use serde_json::json;

/// 与 syncer 同款的最小逻辑帧校验。
fn frame_ok(frame: &Value) -> bool {
    frame.is_object()
        && frame["topic"].is_string()
        && frame["subscriptionId"].is_string()
        && frame["fromSeq"].is_u64()
        && frame["toSeq"].is_u64()
        && matches!(
            frame["payload"]["kind"].as_str(),
            Some("snapshot" | "deltas")
        )
}

fn new_assembler() -> TopicWireFrameAssembler {
    TopicWireFrameAssembler::new(frame_ok)
}

fn complete_frame(ordinal: i64, id: &str, delivery: &str) -> Value {
    json!({
        "wireVersion": 3,
        "kind": "complete",
        "deliveryKind": delivery,
        "logicalFrameId": id,
        "logicalFrameOrdinal": ordinal,
        "topic": "sessions-index//w",
        "subscriptionId": "sub-1",
        "frame": {
            "topic": "sessions-index//w",
            "subscriptionId": "sub-1",
            "fromSeq": 0,
            "toSeq": 3,
            "payload": {"kind": "snapshot", "snapshot": {"logEpoch": "e1", "sessions": []}},
        },
    })
}

/// 把 `logical` 切成 `count` 片生成 wire（可注入坏 checksum / 自定 ordinal/id）。
fn fragment_wire(logical: &[u8], index: usize, ordinal: i64, id: &str, delivery: &str) -> Value {
    let count = 3usize;
    let chunk = logical.len().div_ceil(count);
    json!({
        "wireVersion": 3,
        "kind": "fragment",
        "deliveryKind": delivery,
        "logicalFrameId": id,
        "logicalFrameOrdinal": ordinal,
        "topic": "sessions-index//w",
        "subscriptionId": "sub-1",
        "fragmentIndex": index,
        "fragmentCount": count,
        "logicalBytes": logical.len(),
        "checksum": {"algorithm": "crc32", "value": format!("{:08x}", crc32fast::hash(logical))},
        "dataBase64": base64::engine::general_purpose::STANDARD.encode(&logical[index * chunk..((index + 1) * chunk).min(logical.len())]),
    })
}

fn deltas_payload() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "topic": "sessions-index//w",
        "subscriptionId": "sub-1",
        "fromSeq": 0,
        "toSeq": 1,
        "payload": {"kind": "deltas", "deltas": [{"op": "session.upserted"}]},
    }))
    .unwrap()
}

/// 三片乱序送达，返回 (assembler, 事件批)；`corrupt` 让三片都带同一个错误 checksum
/// （片间不一致会在 accept 期就报 mismatch，到不了完成期 crc32 校验）。
fn assemble(delivery: &str, corrupt: bool) -> (TopicWireFrameAssembler, Vec<AssemblyEvent>) {
    let logical = deltas_payload();
    let mut assembler = new_assembler();
    let mut events = Vec::new();
    for index in 0..3 {
        let mut wire = fragment_wire(&logical, index, 7, "lf-1", delivery);
        if corrupt {
            wire["checksum"]["value"] = json!("deadbeef");
        }
        events.extend(assembler.accept(&wire, 1_000));
    }
    (assembler, events)
}

fn fault_reasons(events: &[AssemblyEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| match event {
            AssemblyEvent::Fault(fault) => Some(fault.reason),
            _ => None,
        })
        .collect()
}

#[test]
fn complete_frames_pass_through_with_their_delivery_kind() {
    let mut assembler = new_assembler();
    let events = assembler.accept(&complete_frame(1, "lf-1", "initial"), 1_000);
    assert!(matches!(
        &events[..],
        [AssemblyEvent::Complete { delivery: Delivery::Initial, frame }]
            if frame["payload"]["kind"] == "snapshot"
    ));
    // 同 ordinal 重放静默（tombstone 生效）。
    assert!(
        assembler
            .accept(&complete_frame(1, "lf-1", "initial"), 1_001)
            .is_empty()
    );
}

#[test]
fn invalid_delivery_and_payload_produce_typed_faults() {
    let mut assembler = new_assembler();
    let events = assembler.accept(&complete_frame(1, "lf-1", "sooner"), 1_000);
    assert_eq!(
        fault_reasons(&events),
        ["proto.frameAssemblyMetadataMismatch"]
    );
    let mut bad_frame = complete_frame(2, "lf-2", "online");
    bad_frame["frame"]["payload"] = json!({"kind": "boom"});
    assert_eq!(
        fault_reasons(&assembler.accept(&bad_frame, 1_001)),
        [INVALID_PAYLOAD]
    );
    // 非 object 的 frame：信封一致检查先于 payload 校验（MISMATCH 而非 INVALID_PAYLOAD）。
    // 前一个 fault 已 tombstone 同 ordinal，换 id + ordinal 才会再次受理。
    bad_frame["frame"] = json!("nope");
    bad_frame["logicalFrameOrdinal"] = json!(3);
    bad_frame["logicalFrameId"] = json!("lf-3");
    assert_eq!(
        fault_reasons(&assembler.accept(&bad_frame, 1_002)),
        ["proto.frameAssemblyMetadataMismatch"]
    );
}

#[test]
fn fragments_assemble_out_of_order_and_settle_the_route() {
    let (mut assembler, events) = assemble("initial", false);
    assert!(matches!(
        &events[..],
        [AssemblyEvent::Complete {
            delivery: Delivery::Initial,
            ..
        }]
    ));
    assert_eq!(
        assembler.next_expiry_at(),
        None,
        "assembly released after completion"
    );
    // 旧 ordinal 的任意片重放静默；同 ordinal 不同 id 才可见。
    let logical = deltas_payload();
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 6, "lf-1", "online"), 1_001)
            .is_empty()
    );
    assert_eq!(
        fault_reasons(&assembler.accept(&fragment_wire(&logical, 0, 7, "lf-7", "online"), 1_002)),
        ["proto.frameAssemblyOrdinalConflict"]
    );
}

#[test]
fn conflicting_duplicate_fragments_fault_with_the_assembly_identity() {
    let logical = deltas_payload();
    let mut assembler = new_assembler();
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 4, "lf-1", "online"), 1)
            .is_empty()
    );
    // 同 index 不同字节 → fragment conflict（fault 带装配身份）。
    let mut conflicting = fragment_wire(&logical, 0, 4, "lf-1", "online");
    conflicting["dataBase64"] =
        json!(base64::engine::general_purpose::STANDARD.encode([9_u8, 9, 9, 9, 9, 9, 9, 9]));
    match &assembler.accept(&conflicting, 2)[..] {
        [AssemblyEvent::Fault(fault)] => {
            assert_eq!(fault.reason, "proto.frameAssemblyFragmentConflict");
            assert_eq!(fault.logical_frame_id, "lf-1");
        }
        other => panic!("conflict fault expected, got {other:?}"),
    }
    // 相同字节重放静默。
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 4, "lf-1", "online"), 3)
            .is_empty()
    );
}

#[test]
fn corrupt_checksum_faults_instead_of_completing() {
    let (_, events) = assemble("online", true);
    assert_eq!(
        fault_reasons(&events),
        ["proto.frameAssemblyChecksumMismatch"]
    );
}

#[test]
fn bad_base64_and_bad_shapes_fault_with_distinct_reasons() {
    let mut wire = json!({
        "wireVersion": 3, "kind": "fragment", "deliveryKind": "online",
        "logicalFrameId": "lf-1", "logicalFrameOrdinal": 1,
        "topic": "sessions-index//w", "subscriptionId": "sub-1",
        "fragmentIndex": 0, "fragmentCount": 2, "logicalBytes": 8,
        "checksum": {"algorithm": "crc32", "value": "00ff00ff"},
        "dataBase64": "not!valid",
    });
    let mut assembler = new_assembler();
    assert_eq!(
        fault_reasons(&assembler.accept(&wire, 1)),
        ["proto.frameAssemblyInvalidBase64"]
    );
    // fault 已写 tombstone：fault 后的重试必须换 ordinal 才会被受理（TS 同）。
    wire["logicalFrameOrdinal"] = json!(2);
    wire["fragmentCount"] = json!(2_000);
    wire["dataBase64"] = json!("AAAA");
    assert_eq!(
        fault_reasons(&assembler.accept(&wire, 2)),
        ["proto.frameFragmentCountExceeded"]
    );
    wire["logicalFrameOrdinal"] = json!(3);
    wire["fragmentCount"] = json!(2);
    wire["checksum"]["value"] = json!("ABCDEF01");
    assert_eq!(
        fault_reasons(&assembler.accept(&wire, 3)),
        ["proto.frameAssemblyMetadataMismatch"],
        "uppercase hex rejected"
    );
}

#[test]
fn late_old_fragments_never_revive_and_ordinal_conflicts_fault() {
    let mut assembler = new_assembler();
    let events = assembler.accept(&complete_frame(5, "lf-5", "online"), 1_000);
    assert!(matches!(events[..], [AssemblyEvent::Complete { .. }]));
    assert!(
        assembler
            .accept(&complete_frame(4, "lf-4", "online"), 1_001)
            .is_empty()
    );
    assert_eq!(
        fault_reasons(&assembler.accept(&complete_frame(5, "lf-other", "online"), 1_002)),
        ["proto.frameAssemblyOrdinalConflict"]
    );
    assert!(matches!(
        assembler.accept(&complete_frame(5, "lf-5", "online"), 1_003)[..],
        []
    ));
}

#[test]
fn non_recovery_supersede_faults_but_recovery_does_not() {
    let logical = deltas_payload();
    let fragment =
        |ordinal: i64, id: &str, delivery: &str| fragment_wire(&logical, 0, ordinal, id, delivery);
    // online 半途被更高 ordinal 的 recovery 取代：无 superseded fault（权威替换）。
    let mut assembler = new_assembler();
    assert!(
        assembler
            .accept(&fragment(3, "lf-3", "online"), 1)
            .is_empty()
    );
    let events = assembler.accept(&fragment(4, "lf-4", "recovery"), 2);
    assert!(fault_reasons(&events).is_empty(), "{events:?}");
    // 非 recovery 的取代：旧 assembly 以 superseded fault 收场。
    let mut assembler = new_assembler();
    assert!(
        assembler
            .accept(&fragment(3, "lf-3", "online"), 1)
            .is_empty()
    );
    let events = assembler.accept(&fragment(4, "lf-4", "online"), 2);
    assert_eq!(fault_reasons(&events), ["proto.frameAssemblySuperseded"]);
}

#[test]
fn a_complete_frame_kills_a_same_route_fragment_assembly() {
    let logical = deltas_payload();
    let mut assembler = new_assembler();
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 2, "lf-1", "online"), 1)
            .is_empty()
    );
    // 同 ordinal 同 id 的 complete：装配以 mismatch fault 收场，complete 自身被丢弃。
    let events = assembler.accept(&complete_frame(2, "lf-1", "online"), 2);
    assert_eq!(
        fault_reasons(&events),
        ["proto.frameAssemblyMetadataMismatch"]
    );
    // tombstone 已写：旧 ordinal 重放静默。
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 2, "lf-1", "online"), 3)
            .is_empty()
    );
}

#[test]
fn stalled_assemblies_expire_with_a_typed_fault() {
    let logical = deltas_payload();
    let mut assembler = new_assembler();
    assembler.accept(&fragment_wire(&logical, 0, 1, "lf-1", "online"), 1_000);
    assert_eq!(
        assembler.next_expiry_at(),
        Some(1_000 + ASSEMBLY_TIMEOUT_MS)
    );
    let events = assembler.accept(
        &complete_frame(9, "lf-9", "online"),
        1_000 + ASSEMBLY_TIMEOUT_MS,
    );
    assert_eq!(fault_reasons(&events), ["proto.frameAssemblyTimedOut"]);
    // 超时后旧 ordinal 重放静默（tombstone 保留在 complete 的 ordinal 9），
    // 同 ordinal 不同 id → 冲突可见。
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 1, "lf-1", "online"), 1_001)
            .is_empty()
    );
    assert_eq!(
        fault_reasons(&assembler.accept(&fragment_wire(&logical, 0, 9, "lf-x", "online"), 1_002)),
        ["proto.frameAssemblyOrdinalConflict"]
    );
}

#[test]
fn abort_releases_bytes_but_keeps_the_tombstone() {
    let logical = deltas_payload();
    let mut assembler = new_assembler();
    assembler.accept(&fragment_wire(&logical, 0, 1, "lf-1", "online"), 1_000);
    assembler.abort("sessions-index//w", "sub-1");
    assert_eq!(assembler.next_expiry_at(), None, "assembly released");
    assert!(
        assembler
            .accept(&fragment_wire(&logical, 0, 1, "lf-1", "online"), 1_001)
            .is_empty()
    );
    assert_eq!(
        fault_reasons(&assembler.accept(&fragment_wire(&logical, 0, 1, "lf-9", "online"), 1_002)),
        ["proto.frameAssemblyOrdinalConflict"],
        "tombstone survives abort"
    );
    assembler.clear();
    assert!(
        matches!(
            assembler.accept(&fragment_wire(&logical, 0, 1, "lf-1", "online"), 1_003)[..],
            []
        ),
        "clear forgets tombstones"
    );
}
