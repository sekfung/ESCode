//! 单 topic 订阅状态机（TS syncer 的 `subscribe{Index,Config}Topic` / `activate*` /
//! `request{Index,Config}Recovery` / `settle*Recovery` / 暂存与重试）。
//!
//! 水位纪律：subscribe ACK 的 epoch/seq 只是 admission metadata，**只有逻辑帧原子 apply 后**
//! 才算 base；delta `fromSeq ≠ seq` 即 gap → 同订阅 resync（带 base）→ ACK 不匹配 / 失败 /
//! 恢复帧超时 → 该 topic 新代 fresh 订阅。订阅前的帧按 (topic, sub) 暂存（1024 帧 / 32MB）。

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::agent_v4::assembler::{AssemblyEvent, Delivery, TopicWireFrameAssembler};
use crate::agent_v4::frames::TopicKind;

use super::{Ingest, now_ms};

/// TS `MAX_PENDING_TOPIC_FRAMES` / `MAX_PENDING_TOPIC_BYTES`。
const MAX_PENDING_FRAMES: usize = 1_024;
const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;
/// TS `PROTOCOL_V4_LIMITS.logicalFrameAssemblyTimeoutMs`（恢复帧收口期限）。
pub(super) const RECOVERY_FRAME_TIMEOUT_MS: u64 = 30_000;

pub(super) struct TopicState {
    pub kind: TopicKind,
    pub topic: String,
    pub subscription_id: Option<String>,
    pub generation: u64,
    pub pending: Option<PendingFrames>,
    /// 每 topic 独立记账的已确认水位（不能共用一个 seq/epoch）。
    pub log_epoch: Option<String>,
    pub seq: u64,
    pub has_applied_base: bool,
    pub recovery: Option<RecoveryState>,
    pub recovery_generation: u64,
    /// 在飞的退避重试任务（suspend 时 abort）。
    pub retry: Option<tokio::task::JoinHandle<()>>,
    pub retry_attempt: u32,
    pub assembler: TopicWireFrameAssembler,
}

pub(super) struct PendingFrames {
    pub frames: Vec<Value>,
    pub staged_bytes: usize,
    pub recovery_needed: bool,
}

#[derive(Clone)]
pub(super) struct RecoveryState {
    pub generation: u64,
    pub force_snapshot: bool,
    pub ack_received: bool,
    pub frame_applied: bool,
    pub upgrade_to_snapshot: bool,
    pub post_recovery_gap_pending: bool,
    /// 收口期限任务已在飞（到期后按 generation 自证，不持有句柄）。
    pub deadline_armed: bool,
}

impl TopicState {
    pub(super) fn new(kind: TopicKind, topic: String, frame_check: fn(&Value) -> bool) -> Self {
        Self {
            kind,
            topic,
            subscription_id: None,
            generation: 0,
            pending: None,
            log_epoch: None,
            seq: 0,
            has_applied_base: false,
            recovery: None,
            recovery_generation: 0,
            retry: None,
            retry_attempt: 0,
            assembler: TopicWireFrameAssembler::new(frame_check),
        }
    }
}

/// 帧泵：hub 监听在 ingest 创建时安装一次，跨 runtime 换代复用（TS `ensureWorkspaceFrameListeners`）。
pub(super) fn install_listeners(ingest: &Arc<Ingest>) {
    for kind in [TopicKind::SessionsIndex, TopicKind::WorkspaceConfig] {
        let mut frames = ingest.base.hub.listen(&ingest.key, kind);
        let ingest = Arc::clone(ingest);
        tokio::spawn(async move {
            loop {
                // 空闲期的装配超时：停滞的分片没有后续帧也要 fault（TS assemblyTimer 的 idle 分支）。
                let next_expiry = ingest
                    .state(kind)
                    .assembler
                    .next_expiry_at()
                    .map(|at| at.saturating_sub(now_ms()));
                let frame = match next_expiry {
                    Some(delay) => {
                        let sleep = tokio::time::sleep(Duration::from_millis(delay));
                        tokio::pin!(sleep);
                        tokio::select! {
                            frame = frames.recv() => frame,
                            () = &mut sleep => {
                                expire_assemblies(&ingest, kind);
                                continue;
                            }
                        }
                    }
                    None => frames.recv().await,
                };
                let Some(frame) = frame else {
                    return;
                };
                handle_wire(&ingest, kind, &frame.params);
            }
        });
    }
}

/// 空闲超时触发的装配淘汰：fault 走与帧内 fault 相同的 gap 推进。
fn expire_assemblies(ingest: &Arc<Ingest>, kind: TopicKind) {
    let events = {
        let mut state = ingest.state(kind);
        state.assembler.expire(now_ms())
    };
    let Some(fault) = events.iter().find_map(|event| match event {
        AssemblyEvent::Fault(fault) => Some(fault.clone()),
        _ => None,
    }) else {
        return;
    };
    eprintln!(
        "escode-host: task index {} physical assembly expired reason={} topic={}",
        kind.as_str(),
        fault.reason,
        fault.topic
    );
    let mut state = ingest.state(kind);
    mark_assembly_fault(ingest, &mut state, fault.delivery);
}

/// TS `handle{SessionsIndex,WorkspaceConfig}Wire`：topic/ownership 过滤 → 暂存或装配。
pub(super) fn handle_wire(ingest: &Arc<Ingest>, kind: TopicKind, wire: &Value) {
    let mut state = ingest.state(kind);
    if wire["topic"].as_str() != Some(state.topic.as_str()) {
        return;
    }
    let Some(subscription_id) = state.subscription_id.clone() else {
        stage_pending(&mut state, wire);
        return;
    };
    if wire["subscriptionId"].as_str() != Some(subscription_id.as_str()) {
        return;
    }
    let events = state.assembler.accept(wire, now_ms());
    let fault = events.iter().find_map(|event| match event {
        AssemblyEvent::Fault(fault) => Some(fault.clone()),
        _ => None,
    });
    if let Some(fault) = fault {
        let topic = state.topic.clone();
        state.assembler.abort(&topic, &subscription_id);
        eprintln!(
            "escode-host: task index {} physical assembly failed reason={} frame={} ordinal={} topic={subscription_id}/{topic}",
            state.kind.as_str(),
            fault.reason,
            fault.logical_frame_id,
            fault.logical_frame_ordinal,
        );
        mark_assembly_fault(ingest, &mut state, fault.delivery);
    } else {
        for event in events {
            if let AssemblyEvent::Complete { delivery, frame } = event {
                super::apply::apply_frame(ingest, &mut state, frame, delivery);
            }
        }
    }
}

/// TS `stagePendingFrame`：订阅建立前的帧有界暂存；超限既不积压也不丢帧——
/// 标记恢复，ACK 后仅为该 topic 发起新代 snapshot 订阅。
fn stage_pending(state: &mut TopicState, wire: &Value) {
    let Some(pending) = state.pending.as_mut() else {
        return;
    };
    if pending.recovery_needed {
        return;
    }
    let bytes = serde_json::to_vec(wire).map_or(usize::MAX, |encoded| encoded.len());
    if bytes > MAX_PENDING_BYTES
        || pending.frames.len() + 1 > MAX_PENDING_FRAMES
        || pending.staged_bytes + bytes > MAX_PENDING_BYTES
    {
        pending.frames.clear();
        pending.staged_bytes = 0;
        pending.recovery_needed = true;
        eprintln!(
            "escode-host: task index {} ACK staging overflow; recovery-needed topic={}",
            state.kind.as_str(),
            state.topic
        );
        return;
    }
    pending.frames.push(wire.clone());
    pending.staged_bytes += bytes;
}

/// TS `markAssemblyFault`：recovery 在飞时 online fault 只记 post-gap；否则按 gap 推进。
pub(super) fn mark_assembly_fault(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    delivery: Option<Delivery>,
) {
    if delivery == Some(Delivery::Online) && state.recovery.is_some() {
        if let Some(recovery) = state.recovery.as_mut() {
            recovery.post_recovery_gap_pending |= recovery.frame_applied;
        }
        return;
    }
    handle_gap(ingest, state, delivery);
}

/// TS `handle{Index,Config}Gap`。
pub(super) fn handle_gap(ingest: &Arc<Ingest>, state: &mut TopicState, delivery: Option<Delivery>) {
    if delivery == Some(Delivery::Recovery) {
        let force_snapshot = state
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.force_snapshot);
        if force_snapshot {
            // 强制 snapshot 仍断档：同订阅流已不可证，改该 topic 新代 snapshot 订阅。
            discard_recovery(state);
            spawn_subscribe(ingest, state.kind, true);
        } else {
            super::topic_subscribe::request_recovery(ingest, state, true);
        }
        return;
    }
    super::topic_subscribe::request_recovery(ingest, state, false);
}

pub(super) fn spawn_subscribe(ingest: &Arc<Ingest>, kind: TopicKind, unsubscribe_active: bool) {
    let ingest = Arc::clone(ingest);
    tokio::spawn(async move {
        super::topic_subscribe::subscribe_topic(&ingest, kind, unsubscribe_active).await
    });
}

/// TS `discard{Index,Config}Recovery`（期限任务靠 generation 自证退出，无需 abort）。
pub(super) fn discard_recovery(state: &mut TopicState) {
    state.recovery = None;
}

/// TS `clear{Index,Config}PendingState`。
pub(super) fn clear_pending(state: &mut TopicState) {
    state.pending = None;
    state.assembler.clear();
}

/// TS `suspendWorkspaceAfterRuntimeUnavailable`：换代失效全部本地态；不发清理 RPC。
pub(super) fn suspend(ingest: &Arc<Ingest>) {
    for kind in [TopicKind::SessionsIndex, TopicKind::WorkspaceConfig] {
        let mut state = ingest.state(kind);
        state.generation += 1;
        state.recovery_generation += 1;
        if let Some(retry) = state.retry.take() {
            retry.abort();
        }
        discard_recovery(&mut state);
        clear_pending(&mut state);
        state.subscription_id = None;
    }
}

/// 恢复帧收口（TS `complete{Index,Config}RecoveryFrame`）。
pub(super) fn complete_recovery_frame(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    delivery: Delivery,
) {
    if delivery != Delivery::Recovery {
        return;
    }
    if let Some(recovery) = state.recovery.as_mut() {
        recovery.frame_applied = true;
    }
    if let Some(recovery) = state.recovery.clone() {
        super::topic_subscribe::settle_recovery(ingest, state, recovery);
    }
}

/// 供单测注入 ACK 后的订阅（不经真实运行时）。
#[cfg(test)]
pub(super) fn set_subscription_for_tests(
    ingest: &Arc<Ingest>,
    kind: TopicKind,
    subscription_id: &str,
    log_epoch: &str,
) {
    let mut state = ingest.state(kind);
    state.subscription_id = Some(subscription_id.to_owned());
    state.log_epoch = Some(log_epoch.to_owned());
    state.pending = Some(PendingFrames {
        frames: Vec::new(),
        staged_bytes: 0,
        recovery_needed: false,
    });
}

/// 供单测读取基线（TS `state.summaries` / `state.seeded`）。
#[cfg(test)]
pub(super) fn core_for_tests(ingest: &Arc<Ingest>) -> std::sync::MutexGuard<'_, super::Core> {
    super::lock(&ingest.core)
}
