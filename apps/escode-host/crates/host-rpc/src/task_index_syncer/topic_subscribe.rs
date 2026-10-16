//! 订阅建立 / resync 恢复 / 退避重试（TS syncer 的 `request*Recovery` / `settle*Recovery` /
//! `schedule*Retry` / `subscribe*Topic` / `activate*Subscribe`）。

use escode_host_control::logging;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agent_v4::base::index_subscribe_body;
use crate::agent_v4::frames::TopicKind;
use crate::forward::{RUNTIME_UNAVAILABLE_CODE, RuntimePolicy};

/// TS `PROVIDER_NOT_READY_RETRY_MS`。
const PROVIDER_NOT_READY_RETRY_MS: u64 = 5_000;
/// TS 瞬态退避：`min(1000, 25 * 2 ** min(attempt, 5))`。
const RETRY_BACKOFF_BASE_MS: u64 = 25;

use super::Ingest;
use super::topic::{PendingFrames, RecoveryState, TopicState, discard_recovery};
use super::topic::{RECOVERY_FRAME_TIMEOUT_MS, clear_pending, handle_wire, spawn_subscribe};

/// TS `request{Index,Config}Recovery`：同订阅 resync（带 base）；无 base 时强制 snapshot。
pub(super) fn request_recovery(ingest: &Arc<Ingest>, state: &mut TopicState, force_snapshot: bool) {
    let Some(subscription_id) = state.subscription_id.clone() else {
        return;
    };
    if let Some(active) = state.recovery.as_mut() {
        if force_snapshot && !active.force_snapshot {
            active.upgrade_to_snapshot = true;
            let recovery = active.clone();
            settle_recovery(ingest, state, recovery);
        }
        return;
    }
    let effective_force = force_snapshot || !state.has_applied_base || state.log_epoch.is_none();
    state.recovery_generation += 1;
    let generation = state.recovery_generation;
    state.recovery = Some(RecoveryState {
        generation,
        force_snapshot: effective_force,
        ack_received: false,
        frame_applied: false,
        upgrade_to_snapshot: false,
        post_recovery_gap_pending: false,
        deadline_armed: false,
    });
    let mut body = json!({
        "topic": state.topic,
        "connectionId": ingest.connection_id,
        "subscriptionId": subscription_id,
    });
    if effective_force {
        body["forceSnapshot"] = Value::Bool(true);
    } else {
        body["base"] = json!({
            "logEpoch": state.log_epoch.clone().unwrap_or_default(),
            "seq": state.seq,
        });
    }
    let ingest = Arc::clone(ingest);
    let kind = state.kind;
    tokio::spawn(async move {
        let answer = ingest
            .base
            .call(
                &ingest.target_json(),
                RuntimePolicy::ExistingOnly,
                "v4/conversation/resync",
                body,
            )
            .await;
        #[derive(PartialEq)]
        enum ResyncOutcome {
            Settle,
            FreshSubscribe,
        }
        let outcome = {
            let mut state = ingest.state(kind);
            if state
                .recovery
                .as_ref()
                .is_none_or(|recovery| recovery.generation != generation)
            {
                return;
            }
            match answer {
                Ok(called) => {
                    let ack = &called.result["ack"];
                    if state.subscription_id.as_deref() != Some(subscription_id.as_str())
                        || ack["subscriptionId"].as_str() != Some(subscription_id.as_str())
                    {
                        // ACK 与当前订阅不匹配：换新鲜订阅。
                        discard_recovery(&mut state);
                        ResyncOutcome::FreshSubscribe
                    } else {
                        if let Some(recovery) = state.recovery.as_mut() {
                            recovery.ack_received = true;
                            recovery.force_snapshot |= ack["mode"] == "snapshot";
                        }
                        if let Some(recovery) = state.recovery.clone() {
                            settle_recovery(&ingest, &mut state, recovery);
                        }
                        ResyncOutcome::Settle
                    }
                }
                Err(_) => {
                    discard_recovery(&mut state);
                    ResyncOutcome::FreshSubscribe
                }
            }
        };
        if outcome == ResyncOutcome::FreshSubscribe {
            subscribe_topic(&ingest, kind, true).await;
        }
    });
}

/// TS `settle{Index,Config}Recovery`：ACK 后按升级/收口/期限推进。
pub(super) fn settle_recovery(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    recovery: RecoveryState,
) {
    if !recovery.ack_received
        || state
            .recovery
            .as_ref()
            .is_none_or(|current| current.generation != recovery.generation)
    {
        return;
    }
    if recovery.upgrade_to_snapshot {
        discard_recovery(state);
        if recovery.force_snapshot {
            spawn_subscribe(ingest, state.kind, true);
        } else {
            request_recovery(ingest, state, true);
        }
        return;
    }
    if recovery.frame_applied {
        let followup = recovery.post_recovery_gap_pending;
        discard_recovery(state);
        if followup {
            request_recovery(ingest, state, false);
        }
        return;
    }
    if recovery.deadline_armed {
        return;
    }
    let ingest = Arc::clone(ingest);
    let kind = state.kind;
    let _deadline = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(RECOVERY_FRAME_TIMEOUT_MS)).await;
        let expired = {
            let mut state = ingest.state(kind);
            let still_current = state
                .recovery
                .as_ref()
                .is_some_and(|current| current.generation == recovery.generation)
                && !recovery.frame_applied;
            if still_current {
                discard_recovery(&mut state);
            }
            still_current
        };
        if !expired {
            return;
        }
        if recovery.force_snapshot {
            subscribe_topic(&ingest, kind, true).await;
        } else {
            let mut state = ingest.state(kind);
            request_recovery(&ingest, &mut state, true);
        }
    });
    if let Some(current) = state.recovery.as_mut() {
        current.deadline_armed = true;
    }
}

/// TS `schedule{Index,Config}Retry`。
pub(super) fn schedule_retry(
    ingest: &Arc<Ingest>,
    state: &mut TopicState,
    provider_not_ready: bool,
) {
    if state.retry.is_some() {
        return;
    }
    let delay = if provider_not_ready {
        PROVIDER_NOT_READY_RETRY_MS
    } else {
        // TS `Math.min(1_000, 25 * 2 ** Math.min(attempt, 5))`。
        RETRY_BACKOFF_BASE_MS
            .saturating_mul(1 << state.retry_attempt.min(5))
            .min(1_000)
    };
    state.retry_attempt += 1;
    let ingest = Arc::clone(ingest);
    let kind = state.kind;
    state.retry = Some(tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let dormant = super::lock(&ingest.runtime_generation).is_none();
        if !dormant {
            subscribe_topic(&ingest, kind, false).await;
        }
    }));
}

/// TS `subscribe{Index,Config}Topic`：换代 → （可选退订旧订阅）→ 订阅 → activate。
pub(super) async fn subscribe_topic(
    ingest: &Arc<Ingest>,
    kind: TopicKind,
    unsubscribe_active: bool,
) {
    let (generation, previous) = {
        let mut state = ingest.state(kind);
        if let Some(retry) = state.retry.take() {
            retry.abort();
        }
        state.generation += 1;
        let generation = state.generation;
        let previous = state.subscription_id.take();
        discard_recovery(&mut state);
        clear_pending(&mut state);
        state.pending = Some(PendingFrames {
            frames: Vec::new(),
            staged_bytes: 0,
            recovery_needed: false,
        });
        let previous = previous.filter(|_| unsubscribe_active);
        (generation, previous)
    };
    if let Some(previous) = previous {
        unsubscribe_subscription(ingest, kind, &previous).await;
        if ingest.state(kind).generation != generation {
            return;
        }
    }
    let body = index_subscribe_body(
        &json!({"visibility": "background"}),
        &ingest.state(kind).topic.clone(),
        &crate::agent_v4::base::Connection {
            connection_id: ingest.connection_id.clone(),
            client_mode: crate::session::ClientMode::DesktopContinuous,
            workflow_run_deltas: false,
        },
    );
    let started = Instant::now();
    let answer = ingest
        .base
        .subscribe(
            &ingest.target_json(),
            RuntimePolicy::ExistingOnly,
            body,
            started,
        )
        .await;
    match answer {
        Ok(subscribed) => activate(ingest, kind, generation, subscribed.result).await,
        Err(error) => {
            let mut state = ingest.state(kind);
            if state.generation != generation {
                return;
            }
            clear_pending(&mut state);
            if error.extra.get("code") == Some(&json!(RUNTIME_UNAVAILABLE_CODE)) {
                // dormant：缺运行时的被动订阅只等下一次 available（TS `runtimeGeneration = null`）。
                *super::lock(&ingest.runtime_generation) = None;
                return;
            }
            let provider_not_ready =
                error.extra.get("code") == Some(&json!("ESCODE_AGENT_PROVIDER_NOT_READY"));
            logging::warn(
                "task-index",
                format!(
                    "{} subscribe failed (provider-not-ready={provider_not_ready}): {}",
                    kind.as_str(),
                    error.message
                ),
            );
            schedule_retry(ingest, &mut state, provider_not_ready);
        }
    }
}

/// TS `activate{Index,Config}Subscribe`。
async fn activate(ingest: &Arc<Ingest>, kind: TopicKind, generation: u64, result: Value) {
    let ack_id = result["ack"]["subscriptionId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let ack_epoch = result["ack"]["logEpoch"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    enum Action {
        /// 迟到旧 ACK（不能反向退订已由新代接管的同 id route）。
        Stale {
            foreign: bool,
        },
        /// ACK 前暂存溢出：退订后换新代订阅。
        Overflow,
        /// 正常激活：释放属于本订阅的暂存帧。
        Apply(Vec<Value>),
        Nothing,
    }
    let action = {
        let mut state = ingest.state(kind);
        if state.generation != generation {
            Action::Stale {
                foreign: state.subscription_id.as_deref() != Some(ack_id.as_str()),
            }
        } else {
            match state.pending.take() {
                None => Action::Nothing,
                Some(pending) if pending.recovery_needed => Action::Overflow,
                Some(pending) => {
                    state.subscription_id = Some(ack_id.clone());
                    state.log_epoch = Some(ack_epoch);
                    state.seq = 0;
                    // 只有逻辑帧原子 apply 后才有 base（ACK 只是 admission metadata）。
                    state.has_applied_base = false;
                    discard_recovery(&mut state);
                    state.retry_attempt = 0;
                    Action::Apply(
                        pending
                            .frames
                            .into_iter()
                            .filter(|frame| frame["subscriptionId"] == ack_id)
                            .collect(),
                    )
                }
            }
        }
    };
    match action {
        Action::Stale { foreign: true } => unsubscribe_subscription(ingest, kind, &ack_id).await,
        Action::Stale { foreign: false } => {}
        Action::Overflow => {
            unsubscribe_subscription(ingest, kind, &ack_id).await;
            if ingest.state(kind).generation == generation {
                spawn_subscribe(ingest, kind, false);
            }
        }
        Action::Apply(frames) => {
            for frame in frames {
                handle_wire(ingest, kind, &frame);
            }
        }
        Action::Nothing => {}
    }
}

async fn unsubscribe_subscription(ingest: &Arc<Ingest>, kind: TopicKind, subscription_id: &str) {
    let topic = ingest.state(kind).topic.clone();
    let body = json!({
        "topic": topic,
        "subscriptionId": subscription_id,
        "connectionId": ingest.connection_id,
    });
    if let Err(error) = ingest
        .base
        .call(
            &ingest.target_json(),
            RuntimePolicy::ExistingOnly,
            "v4/conversation/unsubscribe",
            body,
        )
        .await
    {
        logging::warn(
            "task-index",
            format!(
                "{} unsubscribe cleanup failed: {}",
                kind.as_str(),
                error.message
            ),
        );
    }
}
