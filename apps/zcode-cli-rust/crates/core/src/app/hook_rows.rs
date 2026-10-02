//! Hook 生命周期 → V4 `hookInvocation` 行（TS product-projection `onHookRunLifecycle`）。事件由工作流宿主里的
//! TS hook 运行器发出（docs/specs/rust-hooks.md）：同一次调用（hookInvocationId）一行，按 hookRunId 归并执行明细。
use super::Engine;
use anyhow::Result;
use serde_json::{Map, Value, json};

const SCRIPT_RUNNERS: [&str; 12] = [
    "bash",
    "bun",
    "deno",
    "node",
    "node.exe",
    "powershell",
    "pwsh",
    "python",
    "python3",
    "ruby",
    "sh",
    "zsh",
];
const PROMPT_BLOCK: &str = "hooks_prompt_block";

/// TS 正则 `/"(?:\\.|[^"])*"|'[^']*'|\S+/g` 的切分。
fn tokens(display: &str) -> Vec<String> {
    let chars: Vec<char> = display.chars().collect();
    let (mut out, mut i) = (vec![], 0);
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let quoted = match chars[i] {
            '"' => {
                let mut j = i + 1;
                loop {
                    match chars.get(j) {
                        Some('\\') if j + 1 < chars.len() => j += 2,
                        Some('"') => break Some(j + 1),
                        Some(_) => j += 1,
                        None => break None,
                    }
                }
            }
            '\'' => chars[i + 1..]
                .iter()
                .position(|c| *c == '\'')
                .map(|p| i + p + 2),
            _ => None,
        };
        let end = quoted.unwrap_or_else(|| {
            (i..chars.len())
                .find(|j| chars[*j].is_whitespace())
                .unwrap_or(chars.len())
        });
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    out
}

fn unquote(token: &str) -> String {
    if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
        return serde_json::from_str(token)
            .unwrap_or_else(|_| token[1..token.len() - 1].to_owned());
    }
    if token.len() >= 2 && token.starts_with('\'') && token.ends_with('\'') {
        return token[1..token.len() - 1].to_owned();
    }
    token.to_owned()
}

fn basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or_default()
}

/// TS hookCommandLabel：可执行名（脚本运行器带上脚本名）。
fn command_label(display: &str) -> Option<String> {
    let tokens = tokens(display);
    let executable = basename(&unquote(tokens.first()?)).to_owned();
    if executable.is_empty() {
        return None;
    }
    let Some(script) = tokens.get(1) else {
        return Some(executable);
    };
    if !SCRIPT_RUNNERS.contains(&executable.to_lowercase().as_str()) {
        return Some(executable);
    }
    let script = unquote(script);
    if script.is_empty() || script.starts_with('-') {
        return Some(executable);
    }
    match basename(&script) {
        "" => Some(executable),
        name => Some(format!("{executable} · {name}")),
    }
}

fn display_name(descriptor: &Value, index: u64) -> String {
    let status = descriptor["statusMessage"]
        .as_str()
        .map(str::trim)
        .unwrap_or_default();
    if !status.is_empty() {
        return status.to_owned();
    }
    let executable = descriptor["commandDisplay"]
        .as_str()
        .and_then(command_label);
    let plugin = descriptor["pluginName"].as_str().filter(|s| !s.is_empty());
    match (plugin, executable) {
        (Some(plugin), Some(executable)) => format!("{plugin} · {executable}"),
        (Some(plugin), None) => plugin.to_owned(),
        (None, Some(executable)) => executable,
        (None, None) => format!("Hook #{}", index + 1),
    }
}

fn put(target: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value.filter(|v| !v.is_null() && *v != "") {
        target.insert(key.into(), value);
    }
}

/// 一次生命周期事件 → 执行明细（TS HookExecutionProjection）；不可见 / 内部来源返回 None。
fn execution(kind: &str, payload: &Value, prior: Option<&Value>, at: u64) -> Option<Value> {
    let descriptor = &payload["descriptor"];
    if prior.is_none()
        && (descriptor["clientVisible"] != true || descriptor["sourceKind"] == "internal")
    {
        return None;
    }
    let state = match kind {
        "hook_run_failed" => "failed",
        "hook_run_completed" | "hook_run_blocked" => "completed",
        _ => "running",
    };
    let field = |key: &str| prior.map(|p| p[key].clone()).filter(|v| !v.is_null());
    let started = payload["startedAt"]
        .as_f64()
        .or_else(|| field("startedAt")?.as_f64())
        .unwrap_or(at as f64);
    let ended = (state != "running").then_some(at as f64);
    let duration = payload["durationMs"]
        .as_f64()
        .map(|d| d.max(0.0))
        .or(ended.map(|e| (e - started).max(0.0)));
    let outcome = payload["outcome"].as_str().or(match kind {
        "hook_run_completed" => Some("success"),
        "hook_run_blocked" => Some("blocked"),
        "hook_run_failed" => Some("failed"),
        _ => None,
    });
    let source = field("sourceKind").unwrap_or_else(|| descriptor["sourceKind"].clone());
    if source.is_null() || source == "internal" {
        return None;
    }
    let index = payload["hookIndex"].as_u64()?;
    let mut out = Map::new();
    out.insert("hookRunId".into(), payload["hookRunId"].clone());
    out.insert("hookIndex".into(), index.into());
    let did_execute = field("didExecute") == Some(Value::Bool(true)) || kind == "hook_run_started";
    out.insert("didExecute".into(), did_execute.into());
    out.insert("state".into(), state.into());
    put(&mut out, "outcome", outcome.map(Value::from));
    put(
        &mut out,
        "blockReason",
        payload
            .get("blockReason")
            .cloned()
            .or_else(|| field("blockReason")),
    );
    out.insert("startedAt".into(), json!(started));
    put(&mut out, "endedAt", ended.map(|e| json!(e)));
    put(&mut out, "durationMs", duration.map(|d| json!(d)));
    let name = field("displayName").unwrap_or_else(|| display_name(descriptor, index).into());
    out.insert("displayName".into(), name);
    out.insert("sourceKind".into(), source);
    put(
        &mut out,
        "pluginName",
        field("pluginName").or_else(|| descriptor.get("pluginName").cloned()),
    );
    put(
        &mut out,
        "toolName",
        payload
            .get("toolName")
            .cloned()
            .filter(|v| !v.is_null())
            .or_else(|| field("toolName")),
    );
    Some(Value::Object(out))
}

fn number(value: f64) -> Value {
    if value.fract() == 0.0 {
        json!(value as u64)
    } else {
        json!(value)
    }
}

fn normalize(mut execution: Value) -> Value {
    for key in ["startedAt", "endedAt", "durationMs"] {
        if let Some(v) = execution[key].as_f64() {
            execution[key] = number(v);
        }
    }
    execution
}

impl Engine {
    /// 宿主转发的 `HookRun*` 事件（`at` 是事件时间戳毫秒）。
    pub(super) async fn hook_event(&mut self, id: &str, notice: &Value) -> Result<()> {
        let event = &notice["event"];
        let payload = &event["payload"];
        let (Some(invocation), Some(_), Some(count)) = (
            payload["hookInvocationId"].as_str(),
            payload["hookRunId"].as_str(),
            payload["hookCount"].as_u64().filter(|c| *c > 0),
        ) else {
            return Ok(());
        };
        let kind = event["type"].as_str().unwrap_or_default();
        let at = notice["at"].as_u64().unwrap_or_else(|| self.clock.now());
        let Some(s) = self.sessions.get_mut(id) else {
            return Ok(());
        };
        let existing = s
            .rows
            .iter()
            .rposition(|r| r["kind"] == "hookInvocation" && r["hookInvocationId"] == invocation);
        let previous: Vec<Value> = existing
            .and_then(|i| s.rows[i]["executions"].as_array().cloned())
            .unwrap_or_default();
        let prior = previous
            .iter()
            .find(|e| e["hookRunId"] == payload["hookRunId"]);
        let Some(execution) = execution(kind, payload, prior, at).map(normalize) else {
            return Ok(());
        };
        let did_execute = execution["didExecute"] == true;
        let block = execution["blockReason"].as_str().map(str::to_owned);
        let mut executions: Vec<Value> = previous
            .into_iter()
            .filter(|e| e["hookRunId"] != execution["hookRunId"])
            .collect();
        executions.push(execution);
        executions.sort_by_key(|e| e["hookIndex"].as_u64().unwrap_or(0));
        let row_state = if (executions.len() as u64) < count
            || executions.iter().any(|e| e["state"] == "running")
        {
            "running"
        } else if executions.iter().any(|e| e["state"] == "failed") {
            "failed"
        } else {
            "completed"
        };
        let started = executions
            .iter()
            .filter_map(|e| e["startedAt"].as_f64())
            .fold(f64::MAX, f64::min);
        let event_name = payload["hookEventName"].as_str().unwrap_or_default();
        let lane = match event_name {
            "PreToolUse" | "PermissionRequest" => "toolBefore",
            "PostToolUse" | "PostToolUseFailure" => "toolAfter",
            _ => "assistantWork",
        };
        let mut content = Map::new();
        content.insert("kind".into(), "hookInvocation".into());
        content.insert("hookInvocationId".into(), invocation.into());
        content.insert("hookEventName".into(), event_name.into());
        content.insert("hookCount".into(), count.into());
        content.insert("state".into(), row_state.into());
        content.insert("startedAt".into(), number(started));
        if row_state != "running" {
            let ended = executions
                .iter()
                .filter_map(|e| e["endedAt"].as_f64().or(e["startedAt"].as_f64()))
                .fold(f64::MIN, f64::max);
            content.insert("endedAt".into(), number(ended));
            content.insert("durationMs".into(), number((ended - started).max(0.0)));
        }
        content.insert("lane".into(), lane.into());
        put(
            &mut content,
            "anchorToolCallId",
            payload.get("toolCallId").cloned(),
        );
        content.insert("executions".into(), Value::Array(executions));
        let delta = match existing {
            Some(i) => {
                let row = s.rows[i].as_object_mut().unwrap();
                // TS `{...existingRow, ...content}`：运行中回落时旧的结束时刻保留。
                row.extend(content);
                json!({"op":"row.upserted","row":s.rows[i]})
            }
            None => {
                // 没有回合可挂（会话启动前的 SessionStart 等）：TS 暂存到下一回合；H1 暂不投影。
                let Some(turn) = event["turnId"].as_str().filter(|_| s.running()) else {
                    return Ok(());
                };
                let mut row = s.row("hookInvocation", turn, invocation, at);
                row.as_object_mut().unwrap().extend(content);
                s.rows.push(row.clone());
                json!({"op":"row.appended","row":row})
            }
        };
        // UserPromptSubmit 的执行阻断：作为本次输入的可见错误（TS hookBlockErrorDelta）。
        if kind == "hook_run_blocked"
            && event_name == "UserPromptSubmit"
            && did_execute
            && let Some(reason) = block
        {
            let diagnostic = ["stderrPreview", "errorMessage", "stdoutPreview"]
                .iter()
                .filter_map(|k| payload[*k].as_str().map(str::trim))
                .find(|v| !v.is_empty() && *v != reason);
            let shown = diagnostic.unwrap_or(&reason);
            let message = if shown == PROMPT_BLOCK {
                PROMPT_BLOCK.to_owned()
            } else {
                format!("{PROMPT_BLOCK}: {shown}")
            };
            let mut detail = format!("Hook block reason: {reason}");
            if let Some(d) = diagnostic {
                detail.push_str(&format!("\nHook error: {d}"));
            }
            s.last_error = Some(json!({
                "code": "fault.runtime.hookBlocked", "message": message, "recoverable": false, "at": at,
                "source": "runtime", "traceId": event["traceId"].as_str().unwrap_or_default(), "detail": detail,
                "attribution": {"source": "runtime", "reason": "hook_blocked"},
            }));
        }
        s.revision += 1;
        self.publish(id, vec![delta])?;
        self.persist(id, None).await
    }
}
