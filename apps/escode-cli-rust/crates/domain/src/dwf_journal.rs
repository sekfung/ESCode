//! 动态工作流 run journal 的读面（docs/specs/rust-dynamic-workflow.md 第 4 期前置），对齐 TS
//! `adapters/src/storage/session-store/repositories/dwf-journal-{codecs,introspection,artifacts}.ts`
//! 与 `bootstrap/src/app/dynamic-workflow-run-{artifact-projection,observation}.ts`。
//!
//! 这里只做**纯逻辑**：物理列 → 逻辑记录的解码、产物归并、以及 `workflows/runs` 的行投影。
//! SQL 与 DDL 在 `escode-cli-state`，协议入口在 core。

use crate::json_order::Json;

/// `listRuns` 的查询袋（TS `DwfListRunsQuery`）：`cwd` 缺省即跨项目；`limit` 由调用方钳制
/// （工具面上限 + 1 的截断探测行不许在这里被吃掉）。
#[derive(Clone, Debug, Default)]
pub struct RunQuery {
    pub cwd: Option<String>,
    pub name: Option<String>,
    /// 可选状态子集（逻辑状态词）：缺省即不过滤；**空数组**即「不匹配任何状态」（回空页）。
    pub statuses: Option<Vec<String>>,
    pub limit: i64,
}

/// `dwf_run` 的窄投影行（`DwfRunMetadataRow`）。
#[derive(Clone, Debug)]
pub struct RunRow {
    pub id: String,
    pub parent_session_id: Option<String>,
    pub cwd: Option<String>,
    pub name: Option<String>,
    /// 脚本原文（`ListWorkflowRuns` 的标签派生要用首个非空行）。
    pub script_text: Option<String>,
    /// 修订前驱（lineage 读面的 `resumedFrom`）。
    pub resumed_from: Option<String>,
    pub tool_call_id: Option<String>,
    pub args_json: Option<String>,
    pub spent_tokens: i64,
    pub status: String,
    pub failure_json: Option<String>,
    pub time_created: i64,
    pub time_updated: i64,
}

/// `dwf_node` 的产物行投影（`kind = 'artifact'`；`decodeNode` 后本模块只用到这三列）。
#[derive(Clone, Debug)]
pub struct ArtifactRow {
    pub status: String,
    pub artifact_id: Option<String>,
    pub result_json: Option<String>,
}

/// 一次 `workflows/runs` 要的一行：run 元数据 + 本 run 的产物行（journal 的持久家）。
#[derive(Clone, Debug)]
pub struct JournalRun {
    pub row: RunRow,
    pub artifacts: Vec<ArtifactRow>,
    /// 打了 id 标签的 `report` 行数（预置看板的 `itemCount`）。
    pub reports: Vec<(String, i64)>,
}

/// 逻辑状态（TS `RunStatus`）。
pub const STATUSES: [&str; 5] = ["pending", "running", "completed", "stopped", "errored"];
/// 物理列值（TS `DwfRunPhysicalStatus`）。
const PHYSICAL: [&str; 5] = ["pending", "running", "completed", "failed", "cancelled"];
/// TS `INTERRUPTED_CODE`：物理 `failed` 里区分「中断」与「报错」的唯一标记。
const INTERRUPTED_CODE: &str = "Interrupted";
/// TS `STOP_REASONS`：信封嗅探白名单——少一个值，该原因的整封 envelope 解不出来、行退化成 user。
const STOP_REASONS: [&str; 5] = ["user", "model", "provider", "interrupted", "superseded"];

/// 解码后的 run 元数据（TS `decodeRunMetadata` 去掉 failure/result 之后的部分）。
#[derive(Clone, Debug)]
pub struct Run {
    pub run_id: String,
    pub name: Option<String>,
    pub status: &'static str,
    pub stop_reason: Option<String>,
    pub superseded_by: Option<String>,
    pub parent_session_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub args: Option<Json>,
    pub cwd: Option<String>,
    pub spent_tokens: i64,
    pub time_created: i64,
    pub time_updated: i64,
}

fn physical(status: &str) -> &'static str {
    PHYSICAL.iter().copied().find(|candidate| *candidate == status).unwrap_or("pending")
}

/// JS `Number.isInteger` 语义（`Json` 只有 compact/parse，读数字要自己收窄）。
fn as_int(value: &Json) -> Option<i64> {
    let Json::Number(number) = value else { return None };
    number
        .as_i64()
        .or_else(|| number.as_f64().filter(|float| float.is_finite() && float.fract() == 0.0).map(|float| float as i64))
}

fn is_primary(value: &Json) -> bool {
    value.get("primary") == Some(&Json::Bool(true))
}

fn string_field(value: &Json, key: &str) -> Option<String> {
    value.get(key).and_then(Json::as_str).map(str::to_owned)
}

/// TS `decodeRunSettlement`：物理列 + `failure_json` → 逻辑状态、停留原因与 failure。
fn settlement(status: &str, failure_json: Option<&str>) -> (&'static str, Option<String>, Option<String>) {
    let raw = failure_json.and_then(Json::parse);
    match physical(status) {
        "cancelled" => {
            // 历史无 reason 的 cancelled 一律解成 user（含 TaskStop 停的）。
            let mut reason = "user".to_owned();
            let mut superseded = None;
            let is_envelope = raw
                .as_ref()
                .and_then(|value| value.get("stopReason"))
                .and_then(Json::as_str)
                .is_some_and(|reason| STOP_REASONS.contains(&reason));
            if is_envelope {
                let value = raw.as_ref().expect("信封已在上面判定");
                if let Some(found) = value.get("stopReason").and_then(Json::as_str) {
                    reason = found.to_owned();
                }
                if let Some(found) = value
                    .get("supersededBy")
                    .and_then(Json::as_str)
                    .filter(|text| !text.is_empty())
                {
                    superseded = Some(found.to_owned());
                }
            }
            ("stopped", Some(reason), superseded)
        }
        "failed" => {
            let code = raw.as_ref().and_then(|value| value.get("code")).and_then(Json::as_str);
            if code == Some(INTERRUPTED_CODE) {
                ("stopped", Some("interrupted".to_owned()), None)
            } else {
                ("errored", None, None)
            }
        }
        "completed" => ("completed", None, None),
        other => (if other == "running" { "running" } else { "pending" }, None, None),
    }
}

/// TS `decodeRunMetadata` + 时间戳（`decodeRunListItem`）。
pub fn decode_run(row: &RunRow) -> Run {
    let (status, stop_reason, superseded_by) = settlement(&row.status, row.failure_json.as_deref());
    Run {
        run_id: row.id.clone(),
        name: row.name.clone(),
        status,
        stop_reason,
        superseded_by,
        parent_session_id: row.parent_session_id.clone(),
        tool_call_id: row.tool_call_id.clone(),
        // 早期落库的行该列为 NULL（缺席的键），不是 `{}`。
        args: row.args_json.as_deref().and_then(Json::parse),
        cwd: row.cwd.clone(),
        spent_tokens: row.spent_tokens,
        time_created: row.time_created,
        time_updated: row.time_updated,
    }
}

/// TS `artifactsOf`：只收 `completed` 的产物行，同 id 按版本升序，顶层字段取最新版；
/// 交付物（primary）带头。`itemCount` 由标签 `report` 行数决定（内容产物恒 0）。
pub fn artifacts(rows: &[ArtifactRow], report_counts: Option<&[(String, i64)]>) -> Option<Vec<Json>> {
    let mut order: Vec<String> = Vec::new();
    let mut kinds: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut versions: std::collections::BTreeMap<String, Vec<Json>> = std::collections::BTreeMap::new();
    for row in rows {
        if row.status != "completed" {
            continue;
        }
        let Some(record) = row.result_json.as_deref().and_then(Json::parse) else {
            continue;
        };
        if !record.is_object() {
            continue;
        }
        let Some(version) = version_of(&record) else {
            continue;
        };
        let id = row
            .artifact_id
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| record.get("id").and_then(Json::as_str).map(str::to_owned));
        let Some(id) = id else { continue };
        let Some(kind) = record.get("kind").and_then(Json::as_str).filter(|kind| ARTIFACT_KINDS.contains(kind)) else {
            continue;
        };
        if !kinds.contains_key(&id) {
            order.push(id.clone());
            kinds.insert(id.clone(), kind.to_owned());
        }
        versions.entry(id).or_default().push(version);
    }
    if order.is_empty() {
        return None;
    }
    // 标签计数只在真有**预置看板**时才扫：内容产物的 itemCount 恒 0，而计数是一次全表解码。
    let counts = |id: &str| {
        if !kinds.values().any(|kind| PRESET_KINDS.contains(&kind.as_str())) {
            return 0;
        }
        report_counts
            .and_then(|counts| counts.iter().find(|(name, _)| name == id))
            .map(|(_, count)| *count)
            .unwrap_or(0)
    };
    let mut items = order
        .into_iter()
        .map(|id| {
            let mut sorted = versions.remove(&id).unwrap_or_default();
            sorted.sort_by_key(|version| version.get("version").and_then(as_int).unwrap_or(0));
            let latest = sorted.last().cloned().unwrap_or(Json::Null);
            let primary = sorted.iter().any(is_primary);
            let mut artifact = Json::object();
            artifact.set("id", Json::str(&id));
            artifact.set(
                "kind",
                Json::str(kinds.get(&id).map(String::as_str).unwrap_or_default()),
            );
            for key in ["title", "description", "contentType", "sourcePath", "spec"] {
                if let Some(value) = latest.get(key) {
                    artifact.set(key, value.clone());
                }
            }
            artifact.set(
                "version",
                Json::Number(latest.get("version").and_then(as_int).unwrap_or(0).into()),
            );
            artifact.set("versions", Json::Array(sorted));
            artifact.set("itemCount", Json::Number(counts(&id).into()));
            if primary {
                artifact.set("primary", Json::Bool(true));
            }
            artifact
        })
        .collect::<Vec<_>>();
    // `Array.prototype.sort` 自 ES2019 起稳定：非 primary 之间的相对顺序不动。
    items.sort_by_key(|item| if is_primary(item) { 0 } else { 1 });
    Some(items)
}

/// TS `artifactVersionOf`：`version` 不是正整数就整行丢弃（没有版本号的版本无法定位）。
fn version_of(record: &Json) -> Option<Json> {
    let version = record.get("version").and_then(as_int).filter(|value| *value >= 1)?;
    let mut out = Json::object();
    out.set("version", Json::Number(version.into()));
    for key in ["title", "description", "contentType"] {
        if let Some(value) = string_field(record, key) {
            out.set(key, Json::str(value));
        }
    }
    if let Some(bytes) = record.get("bytes").cloned() {
        // TS 要求 `typeof bytes === "number" && Number.isFinite(bytes)`。
        if matches!(&bytes, Json::Number(number) if number.as_f64().is_some_and(f64::is_finite)) {
            out.set("bytes", bytes);
        }
    }
    if let Some(uri) = string_field(record, "uri") {
        out.set("uri", Json::str(uri));
    }
    if let Some(source) = string_field(record, "sourcePath") {
        out.set("sourcePath", Json::str(source));
    }
    if let Some(spec) = record.get("spec") {
        out.set("spec", spec.clone());
    }
    // driver 恒写 publishedAt；缺席时给 0 而不是丢行。
    let published_at = record.get("publishedAt").and_then(as_int).unwrap_or(0);
    out.set("publishedAt", Json::Number(published_at.into()));
    if is_primary(record) {
        out.set("primary", Json::Bool(true));
    }
    Some(out)
}

const ARTIFACT_KINDS: [&str; 6] = ["file", "markdown", "chart", "table", "metrics", "board"];
/// 预置看板的四个成员：只有它们会被标签 `report` 喂数据。
const PRESET_KINDS: [&str; 4] = ["chart", "table", "metrics", "board"];
/// TS `WORKFLOW_OBSERVATION_DISPLAY_MAX_...`：中枢一页的 chips 上界（协议 schema 的 `.max(8)` 同值）。
pub const PROTOCOL_ARTIFACT_LIMIT: usize = 8;

/// 保序 JSON → `serde_json::Value`（协议返回用；键序已按 TS 对象字面量排好）。
pub fn to_value(value: &Json) -> serde_json::Value {
    serde_json::from_str(&value.compact()).unwrap_or(serde_json::Value::Null)
}

/// 一页 run（已按 `limit + 1` 取）→ 协议行 + `truncated`（TS `listSavedWorkflowRunsOp` 的收尾：
/// 多取的那条只用于判定截断，不进页）。
pub fn protocol_page(runs: &[JournalRun], limit: usize) -> (Vec<Json>, bool) {
    let truncated = runs.len() > limit;
    let page = if truncated { &runs[..limit] } else { runs };
    let rows = page
        .iter()
        .map(|entry| {
            let run = decode_run(&entry.row);
            let artifacts = artifacts(&entry.artifacts, Some(&entry.reports));
            protocol_run(&run, artifacts.as_deref())
        })
        .collect();
    (rows, truncated)
}

/// 一页 run 行 → 协议行（TS `listSavedWorkflowRunsOp` 的行投影）。
pub fn protocol_run(run: &Run, artifacts: Option<&[Json]>) -> Json {
    let mut value = Json::object();
    value.set("runId", Json::str(&run.run_id));
    if let Some(name) = &run.name {
        value.set("name", Json::str(name));
    }
    value.set("status", Json::str(run.status));
    if let Some(reason) = &run.stop_reason {
        value.set("stopReason", Json::str(reason));
    }
    value.set("createdAt", Json::Number(run.time_created.into()));
    value.set("updatedAt", Json::Number(run.time_updated.into()));
    value.set("spentTokens", Json::Number(run.spent_tokens.into()));
    if let Some(parent) = &run.parent_session_id {
        value.set("parentSessionId", Json::str(parent));
    }
    if let Some(call) = &run.tool_call_id {
        value.set("toolCallId", Json::str(call));
    }
    if let Some(args) = &run.args {
        value.set("args", args.clone());
    }
    if let Some(cwd) = &run.cwd {
        value.set("cwd", Json::str(cwd));
    }
    if let Some(artifacts) = artifacts.filter(|items| !items.is_empty()) {
        value.set(
            "artifacts",
            Json::Array(
                artifacts
                    .iter()
                    .take(PROTOCOL_ARTIFACT_LIMIT)
                    .map(|artifact| {
                        let mut row = Json::object();
                        row.set("id", artifact.get("id").cloned().unwrap_or(Json::Null));
                        row.set("kind", artifact.get("kind").cloned().unwrap_or(Json::Null));
                        if let Some(title) = artifact.get("title") {
                            row.set("title", title.clone());
                        }
                        row.set("version", artifact.get("version").cloned().unwrap_or(Json::Null));
                        if let Some(content_type) = artifact.get("contentType") {
                            row.set("contentType", content_type.clone());
                        }
                        row
                    })
                    .collect(),
            ),
        );
    }
    value
}
