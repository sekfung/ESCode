//! model-IO 记录（docs/specs/rust-model-io.md），对齐 TS `adapters/src/model/runner-debug.ts`：
//! 每次 HTTP 尝试一条 `type: "model_io"` 记录，append 到 `~/.escode/cli/{debug,rollout}/model-io-<session>.jsonl`，
//! 供 App「模型调用轨迹」侧栏（`readModelTrajectory`）读取。任何失败都不影响模型请求路径。

use crate::contract::{ModelCallScope, ModelFailure, ModelOutput, model_io_full_retention};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(unused_imports)]
pub(super) use super::model_io_project::{
    iso, millis, project_content, project_message, project_messages, request_body, response,
    tool_call, usage,
};

const MAX_ROLLOUT_FILES: usize = 3;

const MAX_ROLLOUT_SESSION_BYTES: u64 = 64 * 1024 * 1024;

const MAX_DEBUG_SESSION_BYTES: u64 = 256 * 1024 * 1024;

const MAX_ROLLOUT_BASELINE_MESSAGES: usize = 64;

const MAX_DEBUG_BASELINE_MESSAGES: usize = 256;

/// TS `shouldRecordModelIO` + `isDevelopmentModelIOEnv` + `getModelIOBaseDir`：`test` 不写，
/// `development` 写 debug，其余写 rollout。
fn target() -> Option<(PathBuf, bool)> {
    let env = std::env::var("ESCODE_RUNTIME_ENV")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if env == "test" {
        return None;
    }
    let dev = env == "development";
    let home = PathBuf::from(escode_cli_host::credential_cipher::node_homedir());
    let dir = home
        .join(".escode")
        .join("cli")
        .join(if dev { "debug" } else { "rollout" });
    Some((dir, dev))
}

/// 一次 `complete` 调用的请求侧（多次重试尝试共用）。
pub(crate) struct Call {
    scope: ModelCallScope,
    provider_id: String,
    model_id: String,
    max_output_tokens: usize,
    messages: Vec<Value>,
    tool_names: Vec<Value>,
}

impl Call {
    /// 投影在 materialize 附件之前做，`messages` 保持 Rust 内部历史的原貌（不含 base64 展开）。
    pub(crate) fn new(
        provider_id: &str,
        model_id: &str,
        max_output_tokens: usize,
        messages: &[Value],
        tools: &[Value],
    ) -> Option<Self> {
        target()?;
        Some(Self {
            scope: crate::contract::current_model_call(),
            provider_id: provider_id.to_owned(),
            model_id: model_id.to_owned(),
            max_output_tokens,
            messages: project_messages(messages),
            tool_names: tools
                .iter()
                .filter_map(|tool| {
                    tool["function"]["name"]
                        .as_str()
                        .or_else(|| tool["name"].as_str())
                })
                .map(Value::from)
                .collect(),
        })
    }

    /// 写一条尝试记录。`body` 是本次发出的原始请求体字节。
    pub(crate) async fn record(
        &self,
        attempt: u32,
        started: SystemTime,
        body: &[u8],
        result: Result<&ModelOutput, &ModelFailure>,
    ) {
        let completed = SystemTime::now();
        let mut record = Map::new();
        record.insert("type".into(), "model_io".into());
        record.insert("requestId".into(), uuid::Uuid::new_v4().to_string().into());
        record.insert("attempt".into(), attempt.into());
        record.insert("startedAt".into(), iso(started).into());
        record.insert("completedAt".into(), iso(completed).into());
        record.insert(
            "durationMs".into(),
            (millis(completed).saturating_sub(millis(started)) as u64).into(),
        );
        record.insert(
            "model".into(),
            json!({ "modelId": self.model_id, "providerId": self.provider_id }),
        );
        let mut request = Map::new();
        let full = model_io_full_retention();
        let dev = target().is_some_and(|(_, dev)| dev);
        // 生产态成功记录会删掉 body.messages：此时不深解析它（带附件的请求体可达近百 MB）。
        let keep_messages = full || dev || result.is_err();
        if let Some(body) = request_body(body, keep_messages) {
            request.insert("body".into(), body);
        }
        request.insert("maxOutputTokens".into(), self.max_output_tokens.into());
        request.insert("messages".into(), Value::Array(self.messages.clone()));
        request.insert("toolNames".into(), Value::Array(self.tool_names.clone()));
        record.insert("request".into(), Value::Object(request));
        match result {
            Ok(output) => {
                record.insert("response".into(), response(output));
            }
            Err(failure) => {
                record.insert(
                    "error".into(),
                    json!({ "name": failure.code, "message": failure.message }),
                );
            }
        }
        for (key, value) in [
            ("sessionId", self.scope.session_id.as_deref()),
            ("querySource", self.scope.query_source.as_deref()),
            ("turnId", self.scope.turn_id.as_deref()),
        ] {
            if let Some(value) = value {
                record.insert(key.into(), value.into());
            }
        }
        let session = self.scope.session_id.clone();
        let record = Value::Object(record);
        // 同步文件 IO 放到阻塞线程；await 保证同一 session 的尝试按序落盘。
        let _ = tokio::task::spawn_blocking(move || {
            if let Some((dir, dev)) = target() {
                let _ = write(&dir, dev, full, session.as_deref(), record);
            }
        })
        .await;
    }
}

#[derive(Clone, Debug, PartialEq)]
struct CollectionState {
    count: usize,
    samples: Vec<[u8; 32]>,
}

#[derive(Clone, Debug, Default)]
struct State {
    messages: Option<CollectionState>,
    body_messages: Option<CollectionState>,
}

static STATES: LazyLock<Mutex<HashMap<PathBuf, State>>> = LazyLock::new(Default::default);

/// TS `sanitizeFileSegment`：只保留 `[A-Za-z0-9_-]`，连续其余字符折叠为 `-`，去首尾 `-`，截 80。
fn file_segment(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').chars().take(80).collect()
}

fn write(
    dir: &Path,
    dev: bool,
    full: bool,
    session: Option<&str>,
    mut record: Value,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let segment = session.map(file_segment).unwrap_or_default();
    let segment = if segment.is_empty() {
        "no-session".to_owned()
    } else {
        segment
    };
    let path = dir.join(format!("model-io-{segment}.jsonl"));
    let exists = path.exists();
    let mut states = STATES.lock().unwrap_or_else(|e| e.into_inner());
    if full {
        // 全量保留：跳过淘汰、上限、裁剪与压缩；仍更新压缩状态，关闭后下一条 bounded 写入可续接 delta。
        append(&path, &record)?;
        states.insert(path, state_of(&record));
        return Ok(());
    }
    if !dev && !exists {
        rotate(dir, MAX_ROLLOUT_FILES - 1, &mut states);
    }
    let existing = if exists {
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    let limit = if dev {
        MAX_DEBUG_SESSION_BYTES
    } else {
        MAX_ROLLOUT_SESSION_BYTES
    };
    let reset = existing >= limit;
    let has_error = record.get("error").is_some();
    if !dev {
        // 生产 rollout：成功时 body.messages 与 request.messages 重复，删掉；response.body Rust 本就不写。
        if !has_error && let Some(body) = record["request"]["body"].as_object_mut() {
            body.remove("messages");
        }
    }
    let previous = if exists && !reset {
        states.get(&path).cloned()
    } else {
        None
    };
    let state = state_of(&record);
    let baseline = if dev {
        MAX_DEBUG_BASELINE_MESSAGES
    } else {
        MAX_ROLLOUT_BASELINE_MESSAGES
    };
    compact(&mut record, previous.as_ref(), baseline, has_error);
    if reset {
        record["modelIOReset"] = json!({
            "maxFileBytes": limit,
            "previousFileBytes": existing,
            "reason": "session_file_size_limit",
        });
        std::fs::write(&path, format!("{record}\n"))?;
    } else {
        append(&path, &record)?;
    }
    states.insert(path, state);
    Ok(())
}

fn append(path: &Path, record: &Value) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(format!("{record}\n").as_bytes())
}

/// TS `rotateModelIOFiles`：目录内 `model-io-*.jsonl` 按 mtime 从旧到新删到不超过 `max` 个。
fn rotate(dir: &Path, max: usize, states: &mut HashMap<PathBuf, State>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("model-io-") && name.ends_with(".jsonl")
        })
        .map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, entry.path())
        })
        .collect();
    if files.len() <= max {
        return;
    }
    files.sort_by_key(|(modified, _)| *modified);
    let remove = files.len() - max;
    for (_, path) in files.into_iter().take(remove) {
        if std::fs::remove_file(&path).is_ok() {
            states.remove(&path);
        }
    }
}

/// 压缩用的状态取自**写入前（未压缩）**的记录：生产态 body.messages 已删，自然没有 body 状态。
fn state_of(record: &Value) -> State {
    State {
        messages: collection_state(&record["request"]["messages"]),
        body_messages: collection_state(&record["request"]["body"]["messages"]),
    }
}

fn collection_state(value: &Value) -> Option<CollectionState> {
    let items = value.as_array()?;
    Some(CollectionState {
        count: items.len(),
        samples: samples(items, items.len()),
    })
}

/// 首 / 25% / 50% / 75% / 尾 五个采样点的内容指纹（TS 用采样降低误判，这里用内容哈希）。
fn samples(items: &[Value], count: usize) -> Vec<[u8; 32]> {
    if count == 0 {
        return vec![];
    }
    let last = count - 1;
    let mut indexes = vec![0, last / 4, last / 2, last * 3 / 4, last];
    indexes.dedup();
    indexes
        .into_iter()
        .map(|index| Sha256::digest(items[index].to_string().as_bytes()).into())
        .collect()
}

/// TS `compactModelIORequest`：`request.messages` 与 `request.body.messages` 相对上一条做 delta，否则写
/// baseline（超过上限写 `tail`）。计数/类型/偏移写在 `request` 上（body 的用 `bodyMessage*` 前缀）。
fn compact(record: &mut Value, previous: Option<&State>, baseline: usize, has_error: bool) {
    let Some(request) = record["request"].as_object_mut() else {
        return;
    };
    let mut meta = Map::new();
    if let Some(messages) = request.get_mut("messages") {
        compact_collection(
            messages,
            previous.and_then(|p| p.messages.as_ref()),
            ("messageCount", "messagesKind", "messageOffset"),
            baseline,
            &mut meta,
        );
    }
    if let Some(body_messages) = request
        .get_mut("body")
        .and_then(|body| body.get_mut("messages"))
    {
        let keys = ("bodyMessageCount", "bodyMessagesKind", "bodyMessageOffset");
        if has_error && body_messages.is_array() {
            // 失败排障需要完整请求体，不做 delta。
            let len = body_messages.as_array().map_or(0, Vec::len);
            meta.insert(keys.0.into(), len.into());
            meta.insert(keys.1.into(), "full".into());
            meta.insert(keys.2.into(), 0.into());
        } else {
            compact_collection(
                body_messages,
                previous.and_then(|p| p.body_messages.as_ref()),
                keys,
                baseline,
                &mut meta,
            );
        }
    }
    request.extend(meta);
}

fn compact_collection(
    value: &mut Value,
    previous: Option<&CollectionState>,
    (count_key, kind_key, offset_key): (&str, &str, &str),
    baseline: usize,
    meta: &mut Map<String, Value>,
) {
    let Some(items) = value.as_array_mut() else {
        return;
    };
    meta.insert(count_key.into(), items.len().into());
    if let Some(previous) = previous
        .filter(|p| p.count > 0 && items.len() >= p.count && samples(items, p.count) == p.samples)
    {
        items.drain(..previous.count);
        meta.insert(kind_key.into(), "delta".into());
        meta.insert(offset_key.into(), previous.count.into());
        return;
    }
    let baseline = baseline.max(1);
    if items.len() > baseline {
        let offset = items.len() - baseline;
        items.drain(..offset);
        meta.insert(kind_key.into(), "tail".into());
        meta.insert(offset_key.into(), offset.into());
        return;
    }
    meta.insert(kind_key.into(), "full".into());
    meta.insert(offset_key.into(), 0.into());
}

#[cfg(test)]
#[path = "model_io_tests.rs"]
mod tests;
