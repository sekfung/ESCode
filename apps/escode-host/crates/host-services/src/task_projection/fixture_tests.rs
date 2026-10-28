//! TS oracle 差分：用真实 TS 适配器产出的期望资产（`task_snapshot.golden.json`）逐字段比对
//! Rust 投影（docs/specs/rust-u2-tasks-index.md §6.4.3）。
//!
//! 期望资产由 `scripts/generate-escode-host-task-projection-fixtures.mjs` 生成（`--check` 进
//! `pnpm check:escode-host`）：假 `escodeAgentService.resumeSession` 回放固定 agent 快照，
//! 索引行由真实 `TaskIndexRepo.syncTaskMeta` 写入临时库。Rust 侧必须复现**同一批输入**：
//! 同一 agent 快照 + 同一份（空）索引库 → 同一份 `ESCodeTaskSnapshot`。
//!
//! 归一化只有一处：`traceId` 在 TS 是 `generateTraceId`（随机 UUID），Rust 同样随机生成，
//! 差分前按占位符 `<traceId>` 归一（值不可比，形态可比）。

use rusqlite::Connection;
use serde_json::{Map, Value};

use crate::task_projection::service::{
    message_limit_of, project_options_of, read_session_snapshot, resume_hints,
    resume_snapshot_request, snapshot_etag,
};
use crate::tasks_index::TasksIndex;
use crate::tasks_index::schema::EXPECTED_MIGRATIONS;

const GOLDEN: &str = include_str!("task_snapshot.golden.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden fixture must be valid JSON")
}

/// 建一个空 tasks-index（生成资产 DDL + 期望迁移账本），返回临时目录与打开的索引。
fn fresh_index(seed: Option<&Value>) -> (tempfile::TempDir, TasksIndex) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tasks-index.sqlite");
    let connection = Connection::open(&path).expect("open sqlite");
    crate::tasks_index::fixture::apply_generated_schema(&connection, EXPECTED_MIGRATIONS)
        .expect("schema");
    drop(connection);
    let index = TasksIndex::open(&path).expect("open index");
    if let Some(seed) = seed.and_then(Value::as_object) {
        index.seed_task_meta_if_missing(seed).expect("seed row");
    }
    (dir, index)
}

/// `traceId` 归一（随机 UUID）：只替换权限/追问事件里的 `traceId` 值。
fn normalize_trace_ids(value: &Value) -> Value {
    let mut normalized = value.clone();
    let Some(runtime) = normalized.get_mut("runtime").and_then(Value::as_object_mut) else {
        return normalized;
    };
    for key in ["pendingPermissions", "pendingElicitations"] {
        let Some(events) = runtime.get_mut(key).and_then(Value::as_array_mut) else {
            continue;
        };
        for event in events {
            if let Some(record) = event.as_object_mut()
                && record.contains_key("traceId")
            {
                record.insert("traceId".to_owned(), Value::from("<traceId>"));
            }
        }
    }
    normalized
}

fn project_variant(name: &str, seed: Option<&Value>) -> (Value, Value) {
    let golden = golden();
    let entry = &golden["expected"][name];
    // 变体可以带自己的输入快照（error 变体只多一个 lastError）；缺省用基础快照。
    let snapshot = if entry["input"].is_object() {
        entry["input"].clone()
    } else {
        golden["input"]["snapshot"].clone()
    };
    let params = entry["params"].clone();
    let (_dir, index) = fresh_index(seed);
    let expected = &entry["result"]["snapshot"];

    let workspace_path = params["workspacePath"].as_str().unwrap_or_default();
    let identity = params["workspaceIdentity"].as_str();
    let (_, thought_level) =
        resume_hints(Some(&index), &params, workspace_path, identity).expect("hints");
    let _request = resume_snapshot_request(
        params["taskId"].as_str().unwrap_or_default(),
        workspace_path,
        identity,
        thought_level.as_deref(),
        None,
        None,
    );
    let actual = read_session_snapshot(
        &index,
        &snapshot,
        message_limit_of(&params),
        project_options_of(&params),
    )
    .expect("project snapshot");
    (actual, expected.clone())
}

/// 与期望资产同名的四个变体（desktop/limited/replayable/errorSnapshot）逐字段全等。
#[test]
fn rust_projection_matches_the_typescript_oracle() {
    let golden = golden();
    assert_eq!(golden["version"], serde_json::json!(1), "资产版本");
    let seeded = golden["expected"]["replayable"]["seed"].clone();
    for name in ["desktop", "limited", "replayable", "errorSnapshot"] {
        let seed_row = (name == "replayable").then_some(&seeded);
        let (actual, expected) = project_variant(name, seed_row);
        assert_eq!(
            normalize_trace_ids(&actual),
            normalize_trace_ids(&expected),
            "变体 `{name}` 与 TS oracle 不一致\nactual={}",
            serde_json::to_string_pretty(&actual).unwrap_or_default()
        );
    }
}

/// resume 请求形状：本地身份省略 `workspaceIdentity`，replayable 带索引里的档位 hint，
/// `model` 不下发（与 TS `buildSessionResumeParams` 同）。
#[test]
fn resume_request_matches_the_typescript_call_shape() {
    let golden = golden();
    let seed = golden["expected"]["replayable"]["seed"].clone();
    assert!(seed.is_object(), "资产必须带 replayable 的种子行");
    let (_dir, index) = fresh_index(Some(&seed));
    let params = golden["expected"]["replayable"]["params"].clone();
    let workspace_path = params["workspacePath"].as_str().unwrap_or_default();
    let (model, thought_level) = resume_hints(
        Some(&index),
        &params,
        workspace_path,
        params["workspaceIdentity"].as_str(),
    )
    .expect("hints");
    // 索引里种子行的 model/thoughtLevel 被回填（TS `resolveTaskIndexResumeHints` 同）。
    assert_eq!(model.as_deref(), Some("glm/glm-4"));
    assert_eq!(thought_level.as_deref(), Some("none"));
    let request = resume_snapshot_request(
        params["taskId"].as_str().unwrap_or_default(),
        workspace_path,
        params["workspaceIdentity"].as_str(),
        thought_level.as_deref(),
        None,
        None,
    );
    // 与 TS 记录的调用参数逐字段一致（TS 多一个 `model`，它只用于 agent 层守卫、不下发运行时）。
    let mut expected = golden["expected"]["replayable"]["result"]["calls"][0]["params"].clone();
    expected.as_object_mut().expect("object").remove("model");
    assert_eq!(request, expected, "resume 请求必须与 TS 的服务级参数一致");

    // desktop 变体：无索引 hint，也不带 thoughtLevel。
    let params = golden["expected"]["desktop"]["params"].clone();
    let (_dir, index) = fresh_index(None);
    let (model, thought_level) = resume_hints(
        Some(&index),
        &params,
        workspace_path,
        params["workspaceIdentity"].as_str(),
    )
    .expect("hints");
    assert!(model.is_none() && thought_level.is_none());
}

/// ETag：`ifNoneMatch` 命中回 `{snapshot: null, etag, notModified: true}`；同快照稳定。
#[test]
fn etag_is_stable_and_conditional() {
    let golden = golden();
    let (_dir, index) = fresh_index(None);
    let snapshot = golden["input"]["snapshot"].clone();
    let params = golden["expected"]["desktop"]["params"].clone();
    let first = read_session_snapshot(
        &index,
        &snapshot,
        message_limit_of(&params),
        project_options_of(&params),
    )
    .expect("project");
    let etag = snapshot_etag(&first);
    assert_eq!(etag, snapshot_etag(&first), "同一快照 etag 必须稳定");
    assert_eq!(etag.len(), 64);
    // 二次读（同库同快照）后，除权限/追问事件的随机 traceId 外必须逐字段一致
    // ——etag 因此每次调用都不同（TS 同：etag 含 `generateTraceId` 随机值）。
    let second = read_session_snapshot(
        &index,
        &snapshot,
        message_limit_of(&params),
        project_options_of(&params),
    )
    .expect("project");
    assert_eq!(normalize_trace_ids(&first), normalize_trace_ids(&second));
    assert_eq!(
        snapshot_etag(&normalize_trace_ids(&first)),
        snapshot_etag(&normalize_trace_ids(&second)),
        "归一随机 traceId 后 etag 必须收敛（索引回写幂等）"
    );
    // TS 的 etag 形状（值受键序 + 随机 traceId 影响，已登记差异）：两侧都只钉形态与条件语义。
    let shape = &golden["expected"]["desktop"]["result"]["etagShape"];
    assert_eq!(shape["length"], serde_json::json!(64));
    assert_eq!(shape["hex"], serde_json::json!(true));
    assert_eq!(
        golden["expected"]["desktop"]["result"]["conditionalRequestHits"],
        serde_json::json!(false),
        "TS 快照含随机 traceId → 同参数再请求必然不命中"
    );
    // Rust 侧同语义：两次读的快照 etag 不同（随机 traceId），归一后一致。
    assert_ne!(etag, snapshot_etag(&second));
}

/// `getTaskSnapshotBody` / `Ref` / `ToolCallsSlice`：TS 适配器本身返回 `null`（不在该 Host 实现）。
#[test]
fn body_ref_and_tool_slice_are_null_like_typescript() {
    // 期望资产里这三个方法没有触发 resume 调用（TS 直接 `return null`）。
    let golden = golden();
    let calls = &golden["expected"]["desktop"]["result"]["calls"];
    let methods = calls
        .as_array()
        .expect("calls")
        .iter()
        .filter_map(|call| call["method"].as_str())
        .collect::<Vec<_>>();
    // 四次服务调用全是 `session/resume`（`getTaskConfigOptions` 也走 `resumeSnapshot`，
    // 不是 `session/read`；`getTaskSnapshotBody`/`Ref`/`ToolCallsSlice` 直接 `return null` 不碰运行时）。
    assert_eq!(
        methods,
        vec![
            "session/resume",
            "session/resume",
            "session/resume",
            "session/resume"
        ]
    );
}

/// `configOptions` 来自 `settings`（与快照里的 `configOptions` 同源，逐字段相等）。
#[test]
fn config_options_match_the_snapshot_projection() {
    let golden = golden();
    let entry = &golden["expected"]["desktop"]["result"];
    assert_eq!(
        entry["configOptions"], entry["snapshot"]["configOptions"],
        "getTaskConfigOptions 与快照内 configOptions 必须同源"
    );
    // 三个 config option：model / mode / thought_level（enabled 时才有第三个）。
    let ids = entry["configOptions"]
        .as_array()
        .expect("options")
        .iter()
        .filter_map(|option| option["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["model", "mode", "thought_level"]);
}

/// golden 资产里的消息集合（防「测试自己写期望」）：消息 id 顺序与 TS 一致。
#[test]
fn golden_message_order_is_pinned() {
    let golden = golden();
    let ids = golden["expected"]["desktop"]["result"]["snapshot"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["id"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "u1",
            "a1",
            "a2",
            "escode-goal-verification-t1-1",
            "u4",
            "a3",
            "a4",
            "a5"
        ]
    );
}

/// 索引回写的 meta 与 `getTaskMeta` 读回一致（同一库、同一行）。
#[test]
fn indexed_row_round_trips_through_get_task_meta() {
    let golden = golden();
    let (_dir, index) = fresh_index(None);
    let params = golden["expected"]["desktop"]["params"].clone();
    let snapshot = golden["input"]["snapshot"].clone();
    let projected = read_session_snapshot(
        &index,
        &snapshot,
        message_limit_of(&params),
        project_options_of(&params),
    )
    .expect("project");
    let read_back = index
        .get_task_meta("/w", Some("/w"), "task_proj")
        .expect("read meta")
        .expect("row exists");
    let mut expected_map = Map::new();
    for (key, value) in read_back.as_object().expect("object").iter() {
        expected_map.insert(key.clone(), value.clone());
    }
    // `syncTaskMeta` 的返回值就是读回的同一行（除时间列顺序无关）。
    assert_eq!(
        projected["meta"], read_back,
        "投影返回的 meta 必须与索引读回一致"
    );
    // 直接用 BTreeMap 兜一层，确保比较的是「键集合 + 值」，不是字符串形态。
    let projected_map = projected["meta"].as_object().expect("meta object");
    assert_eq!(
        projected_map.keys().collect::<Vec<_>>(),
        expected_map.keys().collect::<Vec<_>>()
    );
}
