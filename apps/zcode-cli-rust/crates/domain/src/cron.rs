//! Cron 工具的校验、Host 请求映射、守卫与输出，逐条对齐 TS `cron.ts` 与 `automation-port.ts`。
//! Host 调用由调用方注入（异步闭包）。见 docs/specs/rust-cron.md。

use std::future::Future;

use serde_json::{Map, Value, json};

use crate::json_order::Json;

pub use crate::cron_input::parse;

pub const MUTATION_TOOLS: [&str; 3] = ["CronCreate", "CronUpdate", "CronDelete"];
const LIMIT_CODE: &str = "AUTOMATION_CREATE_LIMIT_REACHED";
pub const LIMIT_MODEL_MESSAGE: &str = "Automation creation was not performed because the global retained-task limit of 20 was reached. This limit cannot be recovered automatically in the current turn. Do not list, delete, overwrite, retry, or use another tool. Reply once in the user's language that they must manually delete an existing task on the Automations page and then retry.";
const FROM_AUTOMATION_RUN: &str = "Cannot create a scheduled task while running a scheduled task.";
const IN_BOUND_SESSION: &str = "Cannot create a scheduled task inside a session that already belongs to a scheduled task. Ask the user to start a new chat to create another scheduled task.";
const BOUND_CHECK_FAILED: &str =
    "Cannot verify whether this session belongs to a scheduled task. Try again later.";

/// 本轮与会话事实。
pub struct Turn {
    pub automation_turn: bool,
    pub active_automation_id: Option<String>,
    pub bot_delivery_target: Option<Value>,
    pub session_id: String,
    pub mode: String,
    pub model_selection: Option<Value>,
}

pub struct HostError {
    pub code: i64,
    pub message: String,
}

pub struct Outcome {
    /// 成功时的输出与模型可见文案（`JSON.stringify(output)`）。
    pub output: Option<(Value, String)>,
    pub error: Option<String>,
    /// 创建上限：模型看固定文案，结束本轮后续工具。
    pub limit: bool,
    /// CronCreate 成功后冻结的会话标题。
    pub freeze_title: Option<String>,
}

impl Outcome {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            output: None,
            error: Some(message.into()),
            limit: false,
            freeze_title: None,
        }
    }
}

/// TS 自动化轮判定（turn-loop-state `isAutomationMutationRestrictedTurn`）。
pub fn is_automation_turn(automation_id: Option<&str>, disallowed: &[String]) -> bool {
    automation_id.is_some_and(|id| !id.trim().is_empty())
        || MUTATION_TOOLS
            .iter()
            .all(|tool| disallowed.iter().any(|d| d == tool))
}

/// TS `resolveTurnAutomationId`：显式 automationId，否则取 `automation-` 前缀 commandId 的 `:` 之前部分。
pub fn turn_automation_id(explicit: Option<&str>, command_id: &str) -> Option<String> {
    if let Some(id) = explicit.map(str::trim).filter(|id| !id.is_empty()) {
        return Some(id.to_owned());
    }
    let input = command_id.trim();
    if !input.starts_with("automation-") {
        return None;
    }
    let id = input.split(':').next().unwrap_or(input);
    (id.len() > "automation-".len()).then(|| id.to_owned())
}

/// TS `buildTurnToolDisallowlist`（不含 OffPeak）。
pub fn turn_disallowlist(requested: &[String], automation_id: Option<&str>) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    for tool in requested {
        if !tools.contains(tool) {
            tools.push(tool.clone());
        }
    }
    if automation_id.is_some() {
        for tool in MUTATION_TOOLS {
            if !tools.iter().any(|t| t == tool) {
                tools.push(tool.to_owned());
            }
        }
    }
    tools
}

/// TS handler + protocol port 主流程。`call(method, params)` 返回 Host 结果（保持键顺序的 JSON）。
pub async fn run<F, Fut>(tool: &str, input: &Value, turn: &Turn, mut call: F) -> Outcome
where
    F: FnMut(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Json, HostError>>,
{
    if turn.automation_turn && tool != "CronList" {
        return Outcome::failed(format!(
            "{tool} is not allowed while running a scheduled automation."
        ));
    }
    let parsed = match parse(tool, input) {
        Ok(parsed) => parsed,
        Err(message) => return Outcome::failed(message),
    };
    let text = |key: &str| {
        parsed
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match tool {
        "CronList" => match call("automation/list", json!({})).await {
            Ok(result) => {
                let automations = result
                    .get("automations")
                    .and_then(Json::as_array)
                    .unwrap_or_default()
                    .iter()
                    .map(model_automation)
                    .collect();
                let mut output = Json::object();
                output.set("automations", Json::Array(automations));
                success(output, None)
            }
            Err(error) => Outcome::failed(error.message),
        },
        "CronDelete" => {
            let id = text("id");
            match call("automation/delete", json!({"automationId": id})).await {
                Ok(result) => {
                    let deleted = matches!(result.get("deleted"), Some(Json::Bool(true)));
                    let mut output = Json::object();
                    output.set("deleted", Json::Bool(deleted));
                    output.set("id", Json::str(&id));
                    output.set(
                        "message",
                        Json::str(if deleted {
                            format!("Deleted automation {id}.")
                        } else {
                            format!("Automation {id} was not found in the current workspace.")
                        }),
                    );
                    success(output, None)
                }
                Err(error) => Outcome::failed(error.message),
            }
        }
        "CronUpdate" => match call("automation/update", update_params(&parsed)).await {
            Ok(result) => automation_output(&result, "Updated", None),
            Err(error) => Outcome::failed(error.message),
        },
        _ => create(&parsed, turn, &mut call).await,
    }
}

async fn create<F, Fut>(parsed: &Map<String, Value>, turn: &Turn, call: &mut F) -> Outcome
where
    F: FnMut(&'static str, Value) -> Fut,
    Fut: Future<Output = Result<Json, HostError>>,
{
    if turn
        .active_automation_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty())
    {
        return Outcome::failed(FROM_AUTOMATION_RUN);
    }
    let own = &turn.session_id;
    let bound = match call("automation/checkTaskBinding", json!({"targetTaskId": own})).await {
        Ok(result) => Ok(matches!(result.get("bound"), Some(Json::Bool(true)))),
        Err(error) if error.code == -32601 => match call("automation/list", json!({})).await {
            Ok(result) => Ok(result
                .get("automations")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .any(|a| a.get("targetTaskId").and_then(Json::as_str) == Some(own.as_str()))),
            Err(_) => Err(()),
        },
        Err(_) => Err(()),
    };
    match bound {
        Err(()) => return Outcome::failed(BOUND_CHECK_FAILED),
        Ok(true) => return Outcome::failed(IN_BOUND_SESSION),
        Ok(false) => {}
    }
    match call("automation/create", create_params(parsed, turn)).await {
        Ok(result) => {
            let title = result
                .get("automation")
                .and_then(|a| a.get("title"))
                .and_then(Json::as_str)
                .map(|t| crate::web_fetch::js_trim(t).to_owned())
                .filter(|t| !t.is_empty());
            automation_output(&result, "Created", title)
        }
        Err(error) => Outcome {
            limit: error.message.contains(LIMIT_CODE),
            ..Outcome::failed(error.message)
        },
    }
}

fn create_params(p: &Map<String, Value>, turn: &Turn) -> Value {
    let relative = p.get("delayMinutes").filter(|v| v.is_number());
    let carrier = p.contains_key("intervalUnit") && p.contains_key("interval");
    let mut params = Map::new();
    params.insert(
        "cronExpr".into(),
        p.get("cron").cloned().unwrap_or("* * * * *".into()),
    );
    if let Some(delay) = relative {
        params.insert("relativeDelayMinutes".into(), delay.clone());
    }
    params.insert("prompt".into(), p["prompt"].clone());
    params.insert("title".into(), p["title"].clone());
    let recurring = if relative.is_some() {
        false
    } else if carrier {
        true
    } else {
        p.get("recurring").and_then(Value::as_bool).unwrap_or(true)
    };
    params.insert("recurring".into(), recurring.into());
    if let Some(selection) = &turn.model_selection {
        params.insert("modelSelection".into(), selection.clone());
    }
    if !turn.mode.is_empty() {
        let mode = if turn.mode == "auto" {
            "build"
        } else {
            turn.mode.as_str()
        };
        params.insert("mode".into(), mode.into());
    }
    params.insert("targetTaskId".into(), turn.session_id.clone().into());
    if let Some(target) = &turn.bot_delivery_target {
        params.insert("botDeliveryTarget".into(), target.clone());
    }
    if carrier {
        params.insert("intervalUnit".into(), p["intervalUnit"].clone());
        params.insert("interval".into(), p["interval"].clone());
    } else if let Some(max) = p.get("maxRuns") {
        params.insert("maxRuns".into(), max.clone());
    }
    Value::Object(params)
}

fn update_params(p: &Map<String, Value>) -> Value {
    let mut params = Map::new();
    params.insert("automationId".into(), p["id"].clone());
    for (from, to) in [
        ("title", "title"),
        ("cron", "cronExpr"),
        ("prompt", "prompt"),
    ] {
        if let Some(value) = p.get(from) {
            params.insert(to.into(), value.clone());
        }
    }
    if p.contains_key("intervalUnit") && p.contains_key("interval") {
        params.insert("recurring".into(), true.into());
        params.insert("maxRuns".into(), Value::Null);
        params.insert("intervalUnit".into(), p["intervalUnit"].clone());
        params.insert("interval".into(), p["interval"].clone());
    } else {
        for key in ["recurring", "maxRuns"] {
            if let Some(value) = p.get(key) {
                params.insert(key.into(), value.clone());
            }
        }
    }
    Value::Object(params)
}

/// TS `toModelAutomation`：固定字段顺序，缺省字段不输出，嵌套值保持 Host 原顺序。
fn model_automation(automation: &Json) -> Json {
    let mut out = Json::object();
    for key in [
        "automationId",
        "title",
        "cronExpr",
        "prompt",
        "enabled",
        "lifecycleStatus",
        "nextRunAt",
        "lastRunAt",
        "runCount",
        "recurring",
        "maxRuns",
        "scheduleRule",
    ] {
        if let Some(value) = automation.get(key) {
            out.set(key, value.clone());
        }
    }
    out
}

fn automation_output(result: &Json, verb: &str, freeze_title: Option<String>) -> Outcome {
    let automation = result
        .get("automation")
        .cloned()
        .unwrap_or_else(Json::object);
    let id = automation
        .get("automationId")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut output = Json::object();
    output.set("automation", model_automation(&automation));
    output.set("message", Json::str(format!("{verb} automation {id}.")));
    success(output, freeze_title)
}

fn success(output: Json, freeze_title: Option<String>) -> Outcome {
    let content = output.compact();
    let value = serde_json::from_str(&content).unwrap_or(Value::Null);
    Outcome {
        output: Some((value, content)),
        error: None,
        limit: false,
        freeze_title,
    }
}

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;
