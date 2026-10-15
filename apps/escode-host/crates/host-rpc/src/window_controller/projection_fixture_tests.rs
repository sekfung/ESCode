//! TS oracle 帧差分（docs/specs/rust-u2-window-controller.md §4.1）：
//! 重放 `scripts/generate-escode-host-controller-fixtures.mjs` 生成的脚本（由**真实 TS 投影**
//! `windowHostControllerProjection.ts` 驱动），与 Rust 投影的归一化帧序列**有序逐字段**对拍。
//!
//! 归一化只覆盖登记过的随机/时钟值（`subscriptionId` / `logEpoch` / `sentAt`）；批次数、水位
//! `fromSeq`/`toSeq`、增量顺序、行内容与错误文案都必须完全一致。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{FrameSink, LocalScope, Projection, TASKS_TOPIC, WORKSPACES_TOPIC};
use crate::window_controller::rows::row_key;

/// 生成器产物（`node --import tsx scripts/generate-escode-host-controller-fixtures.mjs`）。
const GOLDEN: &str = include_str!("controller_frames.golden.json");

fn lock(frames: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
    std::mem::take(
        &mut *frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// 递归把（订阅号 / logEpoch）替换为占位符；`sentAt` 归零（与生成器同一规则）。
fn normalize(value: &Value, replacements: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::from(
            replacements
                .get(text)
                .cloned()
                .unwrap_or_else(|| text.clone()),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize(item, replacements))
                .collect(),
        ),
        Value::Object(record) => Value::Object(
            record
                .iter()
                .map(|(key, entry)| {
                    (
                        key.clone(),
                        if key == "sentAt" {
                            Value::from(0)
                        } else {
                            normalize(entry, replacements)
                        },
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn scope_of(step: &Value) -> LocalScope {
    LocalScope::new(
        step["workspacePath"].as_str().unwrap_or_default(),
        step["workspaceIdentity"].as_str(),
    )
}

fn memberships_of(scope_step: &Value, scope: &LocalScope) -> BTreeMap<String, Value> {
    scope_step["memberships"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|entry| {
            let task_id = entry["meta"]["taskId"].as_str().unwrap_or_default();
            (
                row_key(
                    &scope.workspace_path,
                    scope.workspace_identity.as_deref(),
                    task_id,
                ),
                entry.clone(),
            )
        })
        .collect()
}

fn overlays_of(scope_step: &Value) -> BTreeMap<String, Value> {
    scope_step["overlays"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|entry| {
            (
                entry["taskId"].as_str().unwrap_or_default().to_owned(),
                entry.clone(),
            )
        })
        .collect()
}

#[test]
fn rust_projection_replays_the_ts_frame_sequence() {
    let fixture: Value = serde_json::from_str(GOLDEN).expect("fixture parses");
    assert_eq!(fixture["version"], 1, "fixture 形状变化时同步升级版本");
    assert_eq!(
        fixture["normalized"],
        serde_json::json!(["subscriptionId", "logEpoch", "sentAt"])
    );

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let projection = Projection::new(Box::new(move || {
        let next = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("rust-id-{next}")
    }));
    let collected: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let mut subscriptions: BTreeMap<String, String> = BTreeMap::new();
    let mut replacements: BTreeMap<String, String> = BTreeMap::new();
    let mut error_indices: Vec<usize> = Vec::new();

    for (index, step) in fixture["script"]
        .as_array()
        .expect("script array")
        .iter()
        .enumerate()
    {
        let op = step["op"].as_str().unwrap_or_default();
        let outcome: Result<(), String> = match op {
            "subscribe" => {
                let topic = step["topic"].as_str().unwrap_or_default().to_owned();
                let subscription_key = step["subscriptionKey"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let collected = Arc::clone(&collected);
                let sink: FrameSink = Arc::new(move |frame| {
                    collected
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(frame);
                });
                projection
                    .subscribe(&topic, sink)
                    .map(|ack| {
                        let subscription_id = ack["ack"]["subscriptionId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                        replacements
                            .insert(subscription_id.clone(), format!("<{subscription_key}>"));
                        subscriptions.insert(subscription_key, subscription_id);
                    })
                    .map_err(|error| error.message)
            }
            "registerSource" => {
                projection.register_source(&scope_of(&step["scope"]));
                Ok(())
            }
            "replaceSnapshot" => {
                let scope = scope_of(&step["scope"]);
                projection.replace_source_snapshot(
                    &scope,
                    memberships_of(step, &scope),
                    overlays_of(step),
                );
                Ok(())
            }
            "replaceOverlays" => {
                let scope = scope_of(&step["scope"]);
                projection.replace_source_overlays(&scope, overlays_of(step));
                Ok(())
            }
            "resync" => {
                let key = step["subscriptionKey"].as_str().unwrap_or_default();
                match subscriptions.get(key) {
                    Some(subscription_id) => projection
                        .resync(subscription_id)
                        .map(|_| ())
                        .map_err(|error| error.message),
                    // 已退订：Rust 侧没有该订阅号（fixture 里 TS 用的 id 已消失）。
                    None => Err("未找到 Controller subscription".to_owned()),
                }
            }
            "unsubscribe" => {
                let key = step["subscriptionKey"].as_str().unwrap_or_default();
                if let Some(subscription_id) = subscriptions.remove(key) {
                    projection.unsubscribe(&subscription_id);
                }
                Ok(())
            }
            "mutate" => {
                let allow_missing = matches!(
                    step["mutation"]["kind"].as_str().unwrap_or_default(),
                    "delete-archived" | "delete-archived-batch"
                );
                projection
                    .validate_mutation(&step["address"], allow_missing)
                    .map(|_| ())
                    .map_err(|error| error.message)
            }
            other => Err(format!("unknown op {other}")),
        };

        match (outcome, step.get("expectError")) {
            (Ok(()), None) => {}
            (Err(message), Some(Value::String(expected))) => {
                assert_eq!(&message, expected, "第 {index} 步错误文案不一致");
                error_indices.push(index);
            }
            // `expectError: true`：TS 侧订阅号已消失（占位 <gone>），只对拍错误前缀。
            (Err(message), Some(Value::Bool(true))) => {
                assert!(
                    message.starts_with("未找到 Controller subscription"),
                    "第 {index} 步错误前缀不一致：{message}"
                );
                error_indices.push(index);
            }
            (Ok(()), Some(expected)) => {
                panic!("第 {index} 步 Rust 成功但 fixture 期望出错：{expected}（step={step}）")
            }
            (Err(message), None) => {
                panic!("第 {index} 步 Rust 出错：{message}（step={step}）")
            }
            (_, Some(other)) => panic!("第 {index} 步 expectError 形状未知：{other}"),
        }
    }

    // logEpoch 与订阅号一样是随机值：按帧里出现的实际值归一为占位符（与生成器同规则）。
    let mut frames = lock(&collected);
    for frame in &frames {
        if let Some(epoch) = frame["logEpoch"].as_str() {
            replacements.insert(epoch.to_owned(), "<logEpoch>".to_owned());
        }
    }
    let normalized: Vec<Value> = frames
        .iter_mut()
        .map(|frame| normalize(frame, &replacements))
        .collect();

    let expected = fixture["frames"].as_array().expect("frames array");
    assert_eq!(normalized.len(), expected.len(), "帧条数不同");
    for (index, (actual, wanted)) in normalized.iter().zip(expected.iter()).enumerate() {
        assert!(
            actual == wanted,
            "第 {index} 帧不一致：\n  rust = {actual}\n  ts   = {wanted}"
        );
    }

    let expected_errors = fixture["errors"].as_array().expect("errors array");
    let expected_indices: Vec<usize> = expected_errors
        .iter()
        .map(|entry| entry["index"].as_u64().unwrap_or_default() as usize)
        .collect();
    assert_eq!(error_indices, expected_indices, "出错步骤下标不一致");

    // topic 字面量与 fixture 一致（防生成器/实现两侧漂移）。
    assert_eq!(TASKS_TOPIC, "controller/tasks-index");
    assert_eq!(WORKSPACES_TOPIC, "controller/workspaces");
}
