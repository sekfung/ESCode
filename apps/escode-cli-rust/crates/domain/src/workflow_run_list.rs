//! `ListWorkflowRuns` 的读面（docs/specs/rust-dynamic-workflow.md 第 6 期），对齐 TS
//! `bootstrap/src/app/dynamic-workflow-run-{observation,label,lineage}.ts` 与
//! `core/src/tool/handlers/list-workflow-runs.ts`（+ `workflow-run-introspection.ts` 的格式化助手）。
//!
//! 这里只做**纯逻辑**：journal 行 + 本会话 id → 列表项（标签、归属、`possiblyInterrupted`）、
//! 工具输出与两个面。活注册表（submit → createRun 间隙的条目）在 Rust 侧还不存在（引擎是第 4 期），
//! 所以本模块实现的是「注册表为空」这一支——TS 同款分支，逐字对齐。
use crate::dwf_journal::{self, RunRow};
use crate::json_order::Json;

/// 派生标签的字符上限（TS `DYNAMIC_WORKFLOW_RUN_LABEL_MAX_CHARS`）。
const LABEL_MAX_CHARS: usize = 80;
/// 工具通道预算（TS `LIST_WORKFLOW_RUNS_MODEL_BYTES`）。
const MODEL_BYTES: usize = 24_000;

/// 終态集（TS `TERMINAL_RUN_STATUSES`）：`possiblyInterrupted` 与停止原因的判据。
pub const TERMINAL: [&str; 3] = ["completed", "errored", "stopped"];

/// 列表项（TS `ListWorkflowRunsRun`）。
#[derive(Clone, Debug)]
pub struct Item {
    pub run_id: String,
    pub label: String,
    /// `"name"` = 用户起的名字；`"script"` = 读时从脚本派生（含 runId 兜底）。
    pub label_source: &'static str,
    pub status: &'static str,
    pub stop_reason: Option<String>,
    pub resumed_from: Option<String>,
    pub superseded_by: Option<String>,
    pub owned_by_this_session: bool,
    pub possibly_interrupted: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub spent_tokens: i64,
}

/// TS `resolveDynamicWorkflowRunLabel`：`name` → 脚本首个非空行（trim 后截 80）→ runId。
/// 派生结果**绝不回写** `dwf_run.name`（读时派生）。
pub fn label(run_id: &str, name: Option<&str>, script_text: Option<&str>) -> (String, &'static str) {
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
        return (name.to_owned(), "name");
    }
    if let Some(line) = script_text.and_then(first_non_empty_line) {
        return (bound_label(line), "script");
    }
    (run_id.to_owned(), "script")
}

fn first_non_empty_line(script_text: &str) -> Option<&str> {
    script_text
        .split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())
}

/// 截到上限，且绝不留下孤立代理项（脚本是模型写的外部输入，字面量里可以有 emoji）。
fn bound_label(value: &str) -> String {
    if value.encode_utf16().count() <= LABEL_MAX_CHARS {
        return value.to_owned();
    }
    let mut used = 0usize;
    let mut out = String::new();
    for c in value.chars() {
        if used + c.len_utf16() > LABEL_MAX_CHARS {
            break;
        }
        used += c.len_utf16();
        out.push(c);
    }
    out
}

/// TS `journalRunSummary`（注册表为空那一支）：journal 行 + 本会话 id → 列表项。
pub fn item(row: &RunRow, owner_session: &str) -> Item {
    let run = dwf_journal::decode_run(row);
    let owned = row.parent_session_id.as_deref() == Some(owner_session);
    // 读的是 journal 的状态：语义就是「journal 说它还没结束，而本会话无法证实」。
    let possibly_interrupted = !TERMINAL.contains(&run.status) && !owned;
    let stop_reason = if run.status == "stopped" {
        Some(run.stop_reason.clone().unwrap_or_else(|| "user".to_owned()))
    } else {
        None
    };
    // 后继指针只对 stopped 有意义：其余状态上即便载荷带着这个键，也读作缺席。
    let superseded_by = if run.status == "stopped" { run.superseded_by } else { None };
    let (label, label_source) = label(&run.run_id, run.name.as_deref(), row.script_text.as_deref());
    Item {
        run_id: run.run_id,
        label,
        label_source,
        status: run.status,
        stop_reason,
        resumed_from: row.resumed_from.clone(),
        superseded_by,
        owned_by_this_session: owned,
        possibly_interrupted,
        created_at: run.time_created,
        updated_at: run.time_updated,
        spent_tokens: run.spent_tokens,
    }
}

/// TS `ListWorkflowRunsOutput`：`runs` + 为真才在场的 `truncated`。
pub fn output(items: &[Item], truncated: bool) -> Json {
    let mut value = Json::object();
    value.set(
        "runs",
        Json::Array(
            items
                .iter()
                .map(|item| {
                    let mut run = Json::object();
                    run.set("runId", Json::str(&item.run_id));
                    run.set("label", Json::str(&item.label));
                    run.set("labelSource", Json::str(item.label_source));
                    run.set("status", Json::str(item.status));
                    if let Some(reason) = &item.stop_reason {
                        run.set("stopReason", Json::str(reason));
                    }
                    if let Some(from) = &item.resumed_from {
                        run.set("resumedFrom", Json::str(from));
                    }
                    if let Some(by) = &item.superseded_by {
                        run.set("supersededBy", Json::str(by));
                    }
                    run.set("ownedByThisSession", Json::Bool(item.owned_by_this_session));
                    if item.possibly_interrupted {
                        run.set("possiblyInterrupted", Json::Bool(true));
                    }
                    run.set("createdAt", number(item.created_at));
                    run.set("updatedAt", number(item.updated_at));
                    run.set("spentTokens", number(item.spent_tokens));
                    run
                })
                .collect(),
        ),
    );
    if truncated {
        value.set("truncated", Json::Bool(true));
    }
    value
}

/// TS `formatListWorkflowRunsModelContent`：一个 XML-ish 容器 + 一 run 一行（属性式）。
pub fn format_model_content(output: &Json) -> String {
    let runs = output.get("runs").and_then(Json::as_array).unwrap_or_default();
    let truncated = output.get("truncated") == Some(&Json::Bool(true));
    let header = if truncated {
        format!("{} {}", attribute("count", &runs.len().to_string()), attribute("truncated", "true"))
    } else {
        attribute("count", &runs.len().to_string())
    };
    if runs.is_empty() {
        // 「这个项目没跑过 workflow」必须说成一句话：空容器容易被读成「工具没答上来」。
        return format!(
            "<workflow_runs {header}>\nNo workflow runs recorded for this project.\n</workflow_runs>"
        );
    }
    let rows = runs.iter().map(|run| {
        let mut row = vec!["<run".to_owned()];
        let text = |key: &str| run.get(key).and_then(Json::as_str);
        for (name, key) in [
            ("id", "runId"),
            ("status", "status"),
            ("stop_reason", "stopReason"),
            ("resumed_from", "resumedFrom"),
            ("superseded_by", "supersededBy"),
            ("label", "label"),
            ("label_source", "labelSource"),
        ] {
            if let Some(value) = text(key) {
                row.push(attribute(name, value));
            }
        }
        row.push(attribute(
            "owned_by_this_session",
            if run.get("ownedByThisSession") == Some(&Json::Bool(true)) {
                "true"
            } else {
                "false"
            },
        ));
        if run.get("possiblyInterrupted") == Some(&Json::Bool(true)) {
            row.push(attribute("possibly_interrupted", "true"));
        }
        row.push(attribute(
            "spent_tokens",
            &run.get("spentTokens").and_then(as_int).unwrap_or(0).to_string(),
        ));
        row.push(attribute(
            "created_at",
            &timestamp(run.get("createdAt").and_then(as_int).unwrap_or(0)),
        ));
        row.push(attribute(
            "updated_at",
            &timestamp(run.get("updatedAt").and_then(as_int).unwrap_or(0)),
        ));
        row.push("/>".to_owned());
        row.join(" ")
    });
    [
        format!("<workflow_runs {header}>"),
        rows.collect::<Vec<_>>().join("\n"),
        "</workflow_runs>".to_owned(),
    ]
    .join("\n")
}

/// 工具通道的模型内容：格式化结果再按 `resultBudget` 截断（同 ListSavedWorkflows）。
pub fn model_content(output: &Json) -> String {
    crate::tool_display::truncate_model_content(format_model_content(output), MODEL_BYTES)
}

/// TS `createListWorkflowRunsDisplay`（kind `list_workflow_runs`）：行已全部已有界，原样透传，
/// 截断语义由输出自身的 `truncated` 表达。
pub fn display(output: &Json) -> Json {
    let mut value = Json::object();
    value.set("kind", Json::str("list_workflow_runs"));
    value.set(
        "runs",
        output.get("runs").cloned().unwrap_or(Json::Array(Vec::new())),
    );
    if output.get("truncated") == Some(&Json::Bool(true)) {
        value.set("truncated", Json::Bool(true));
    }
    value
}

/// TS `workflowRunAttribute`：值先把空白折成单空格再转义（属性里的换行会破坏「一 run 一行」）。
fn attribute(name: &str, value: &str) -> String {
    let text = value.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{name}=\"{}\"", escape_xml(&text))
}

/// TS `escapeXml`（runtime-task 的通知投影，两处共用同一张转义表）。
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn number(value: i64) -> Json {
    Json::Number(value.into())
}

fn as_int(value: &Json) -> Option<i64> {
    match value {
        Json::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0).map(|f| f as i64)),
        _ => None,
    }
}

/// TS `formatWorkflowRunTimestamp`：epoch ms → ISO 8601（UTC，毫秒三位）；越界值落回原数字。
pub fn timestamp(epoch_ms: i64) -> String {
    if !(-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&epoch_ms) {
        return epoch_ms.to_string();
    }
    let days = epoch_ms.div_euclid(86_400_000);
    let rest = epoch_ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let (hours, minutes, seconds, millis) = (
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000,
    );
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

/// Howard Hinnant 的 days-from-civil 逆算法（与 JS `Date` 的 UTC 日历一致）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}
