//! 待处理权限 → 流事件的投影（task facade 的 `runtime.pendingPermissions` /
//! `runtime.pendingElicitations`）。
//!
//! 上游真相源：`packages/services/src/escode-agent/escodeTaskServiceAdapter.ts` 的
//! `pendingPermissionToStreamEvent`、`pendingUserInputBackedPermissionToElicitationEvent`、
//! `pendingAskUserQuestionToElicitationEvent`、`pendingExitPlanModeToElicitationEvent`、
//! `askUserQuestionInputToElicitationQuestions`、`createExitPlanModeApprovalQuestions`、
//! `isUserInputBackedPermissionToolName`。
//!
//! `traceId` 在 TS 是 `generateTraceId(taskId)`（随机 UUID）；这里同样生成 UUID v4，
//! 值不可比（差分时按占位符归一）。

use serde_json::{Map, Value};

pub const ASK_USER_QUESTION_TOOL_NAME: &str = "AskUserQuestion";
pub const EXIT_PLAN_MODE_TOOL_NAME: &str = "ExitPlanMode";
pub const EXIT_PLAN_MODE_APPROVAL_QUESTION: &str = "Review this implementation plan.";
pub const EXIT_PLAN_MODE_APPROVAL_APPROVE: &str = "approve";

/// TS `isUserInputBackedPermissionToolName`：需要折成 elicitation 的工具。
pub fn is_user_input_backed_permission_tool_name(value: &str) -> bool {
    value == ASK_USER_QUESTION_TOOL_NAME || value == EXIT_PLAN_MODE_TOOL_NAME
}

/// TS `pendingPermissionToStreamEvent`（普通工具权限请求）。
pub fn pending_permission_to_stream_event(task_id: &str, permission: &Value) -> Value {
    let tool_name = string_field(permission, "toolName");
    let mut event = Map::new();
    event.insert("type".to_owned(), Value::from("permission_request"));
    event.insert("taskId".to_owned(), Value::from(task_id));
    event.insert("traceId".to_owned(), Value::from(random_trace_id()));
    insert_optional(
        &mut event,
        "requestId",
        string_field(permission, "requestId"),
    );
    // TS `permission.reason || permission.toolName`（两侧都无值时键消失）。
    insert_optional(
        &mut event,
        "description",
        string_field(permission, "reason").or_else(|| tool_name.clone()),
    );
    insert_optional(&mut event, "kind", tool_name.clone());
    insert_optional(&mut event, "title", tool_name);
    if let Some(options) = field(permission, "options") {
        event.insert("options".to_owned(), options.clone());
    }
    insert_optional(&mut event, "origin", string_field(permission, "origin"));
    event.insert("raw".to_owned(), permission.clone());
    Value::Object(event)
}

/// TS `pendingUserInputBackedPermissionToElicitationEvent`：非 user-input 工具回 `None`。
pub fn pending_user_input_backed_permission_to_elicitation_event(
    task_id: &str,
    permission: &Value,
) -> Option<Value> {
    let tool_name = string_field(permission, "toolName").unwrap_or_default();
    if tool_name == ASK_USER_QUESTION_TOOL_NAME {
        return ask_user_question_to_elicitation_event(task_id, permission);
    }
    if tool_name == EXIT_PLAN_MODE_TOOL_NAME {
        return Some(exit_plan_mode_to_elicitation_event(task_id, permission));
    }
    None
}

/// TS `pendingAskUserQuestionToElicitationEvent`：问题列表解析不出来就回 `null`。
fn ask_user_question_to_elicitation_event(task_id: &str, permission: &Value) -> Option<Value> {
    let questions = ask_user_question_options(field(permission, "input")?);
    if questions.is_empty() {
        // TS `if (questions.length === 0) return null`：解析不出候选就不发 elicitation。
        return None;
    }
    let first = questions.first();
    let mut event = Map::new();
    event.insert("type".to_owned(), Value::from("elicitation_request"));
    event.insert("taskId".to_owned(), Value::from(task_id));
    event.insert("traceId".to_owned(), Value::from(random_trace_id()));
    insert_optional(
        &mut event,
        "requestId",
        string_field(permission, "requestId"),
    );
    event.insert(
        "message".to_owned(),
        Value::from(
            first
                .and_then(|question| string_field(question, "question"))
                .or_else(|| string_field(permission, "reason"))
                .unwrap_or_default(),
        ),
    );
    insert_optional(
        &mut event,
        "header",
        first.and_then(|question| string_field(question, "header")),
    );
    event.insert(
        "options".to_owned(),
        first
            .and_then(|question| field(question, "options"))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    if first
        .and_then(|question| field(question, "multiSelect"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        event.insert("multiSelect".to_owned(), Value::Bool(true));
    }
    event.insert("questions".to_owned(), Value::Array(questions));
    insert_optional(&mut event, "origin", string_field(permission, "origin"));
    if let Some(input) = field(permission, "input") {
        event.insert("schema".to_owned(), input.clone());
    }
    Some(Value::Object(event))
}

/// TS `pendingExitPlanModeToElicitationEvent`：审批问题固定一条，计划正文只带 `plan`。
fn exit_plan_mode_to_elicitation_event(task_id: &str, permission: &Value) -> Value {
    let questions = exit_plan_mode_approval_questions();
    let first = &questions[0];
    let plan = field(permission, "input")
        .and_then(|input| field(input, "plan"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty());

    let mut event = Map::new();
    event.insert("type".to_owned(), Value::from("elicitation_request"));
    event.insert("taskId".to_owned(), Value::from(task_id));
    event.insert("traceId".to_owned(), Value::from(random_trace_id()));
    insert_optional(
        &mut event,
        "requestId",
        string_field(permission, "requestId"),
    );
    event.insert(
        "message".to_owned(),
        Value::from(EXIT_PLAN_MODE_APPROVAL_QUESTION),
    );
    event.insert(
        "header".to_owned(),
        first
            .get("header")
            .cloned()
            .unwrap_or_else(|| Value::from("Plan")),
    );
    event.insert(
        "options".to_owned(),
        first
            .get("options")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    event.insert("questions".to_owned(), Value::Array(questions));
    insert_optional(&mut event, "origin", string_field(permission, "origin"));
    // 计划审批投影必须展示本次 ExitPlanMode 对应的计划正文；只定向投影 plan，
    // 避免把其他 permission input 泄漏到通用 elicitation schema。
    let mut schema = Map::new();
    schema.insert("interaction".to_owned(), Value::from("plan_approval"));
    insert_optional(
        &mut schema,
        "toolName",
        string_field(permission, "toolName"),
    );
    if let Some(plan) = plan {
        schema.insert("plan".to_owned(), Value::from(plan));
    }
    event.insert("schema".to_owned(), Value::Object(schema));
    Value::Object(event)
}

/// TS `createExitPlanModeApprovalQuestions`。
fn exit_plan_mode_approval_questions() -> Vec<Value> {
    vec![serde_json::json!({
        "header": "Plan",
        "options": [{
            "description": "Exit plan mode and start implementation.",
            "label": "Approve",
            "value": EXIT_PLAN_MODE_APPROVAL_APPROVE,
        }],
        "question": EXIT_PLAN_MODE_APPROVAL_QUESTION,
    })]
}

/// TS `askUserQuestionInputToElicitationQuestions`：label 缺失/空问题被丢弃。
fn ask_user_question_options(input: &Value) -> Vec<Value> {
    let Some(questions) = field(input, "questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    questions
        .iter()
        .filter_map(|question| {
            let question_text = string_field(question, "question");
            let header = string_field(question, "header").or_else(|| question_text.clone());
            let options = field(question, "options")
                .and_then(Value::as_array)
                .map(|options| {
                    options
                        .iter()
                        .filter_map(|option| {
                            let label = string_field(option, "label")?;
                            let mut parsed = Map::new();
                            parsed.insert("value".to_owned(), Value::from(label.clone()));
                            parsed.insert("label".to_owned(), Value::from(label));
                            insert_optional(
                                &mut parsed,
                                "description",
                                string_field(option, "description"),
                            );
                            Some(Value::Object(parsed))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let question_text = question_text?;
            let header = header?;
            if options.is_empty() {
                return None;
            }
            let mut parsed = Map::new();
            parsed.insert("question".to_owned(), Value::from(question_text));
            parsed.insert("header".to_owned(), Value::from(header));
            parsed.insert("options".to_owned(), Value::Array(options));
            if field(question, "multiSelect").and_then(Value::as_bool) == Some(true) {
                parsed.insert("multiSelect".to_owned(), Value::Bool(true));
            }
            Some(Value::Object(parsed))
        })
        .collect()
}

/// TS `generateTraceId(taskId)`：`createUuid()`（UUID v4）。
fn random_trace_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    let text = field(value, key)?.as_str()?;
    (!text.is_empty()).then(|| text.to_owned())
}

fn insert_optional(record: &mut Map<String, Value>, key: &str, value: Option<String>) {
    let Some(value) = value else {
        return;
    };
    if value.is_empty() {
        return;
    }
    record.insert(key.to_owned(), Value::from(value));
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|record| record.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permission_request_keeps_reason_fallback_and_origin() {
        let event = pending_permission_to_stream_event(
            "task_1",
            &json!({"requestId": "req_1", "toolName": "Bash", "reason": "", "options": [{"value": "once"}],
                    "origin": "runtime", "input": {"command": "ls"}}),
        );
        assert_eq!(event["type"], json!("permission_request"));
        assert_eq!(event["taskId"], json!("task_1"));
        // reason 为空串时回退工具名。
        assert_eq!(event["description"], json!("Bash"));
        assert_eq!(event["kind"], json!("Bash"));
        assert_eq!(event["title"], json!("Bash"));
        assert_eq!(event["origin"], json!("runtime"));
        assert_eq!(event["options"], json!([{"value": "once"}]));
        assert_eq!(event["raw"]["input"], json!({"command": "ls"}));
        assert!(
            event["traceId"].as_str().is_some_and(|id| id.len() == 36),
            "traceId 必须是 UUID 形态：{event}"
        );
        // options / origin 缺失时键消失（TS 对象字面量里是 undefined）。
        let bare = pending_permission_to_stream_event("t", &json!({"toolName": "Read"}));
        assert!(bare.get("options").is_none());
        assert!(bare.get("origin").is_none());
        assert_eq!(bare["description"], json!("Read"));
        // toolName 与 reason 都缺失时连 description 也不写。
        let nameless = pending_permission_to_stream_event("t", &json!({}));
        assert!(nameless.get("description").is_none());
        assert!(nameless.get("kind").is_none());
    }

    #[test]
    fn ask_user_question_maps_to_elicitation_with_questions() {
        let permission = json!({
            "requestId": "req_q", "toolName": "AskUserQuestion", "reason": "选择一项",
            "origin": "runtime",
            "input": {"questions": [
                {"question": "哪个？", "header": "选择", "multiSelect": true,
                 "options": [{"label": "A", "description": "第一个"}, {"label": "B"}]},
                {"question": "缺选项", "options": []},
            ]},
        });
        let event =
            pending_user_input_backed_permission_to_elicitation_event("task_1", &permission)
                .expect("elicitation");
        assert_eq!(event["type"], json!("elicitation_request"));
        assert_eq!(event["message"], json!("哪个？"));
        assert_eq!(event["header"], json!("选择"));
        assert_eq!(event["multiSelect"], json!(true));
        assert_eq!(
            event["options"],
            json!([{"value": "A", "label": "A", "description": "第一个"}, {"value": "B", "label": "B"}])
        );
        assert_eq!(event["questions"].as_array().expect("questions").len(), 1);
        assert_eq!(event["schema"], permission["input"]);
        // 普通工具：不是 elicitation。
        assert!(
            pending_user_input_backed_permission_to_elicitation_event(
                "t",
                &json!({"toolName": "Bash"})
            )
            .is_none()
        );
        // questions 解析不出来（AskUserQuestion）→ null。
        assert!(
            pending_user_input_backed_permission_to_elicitation_event(
                "t",
                &json!({"toolName": "AskUserQuestion", "input": {"questions": []}})
            )
            .is_none()
        );
    }

    #[test]
    fn exit_plan_mode_carries_plan_text_and_fixed_approval_question() {
        let event = pending_user_input_backed_permission_to_elicitation_event(
            "task_1",
            &json!({"requestId": "req_p", "toolName": "ExitPlanMode",
                    "input": {"plan": "  步骤一  ", "other": "x"}}),
        )
        .expect("elicitation");
        assert_eq!(event["message"], json!(EXIT_PLAN_MODE_APPROVAL_QUESTION));
        assert_eq!(event["header"], json!("Plan"));
        assert_eq!(event["schema"]["interaction"], json!("plan_approval"));
        assert_eq!(event["schema"]["toolName"], json!("ExitPlanMode"));
        // plan 取 trim 后的正文；其他 input 字段不泄漏。
        assert_eq!(event["schema"]["plan"], json!("步骤一"));
        assert!(event["schema"].get("other").is_none());
        assert_eq!(event["options"][0]["value"], json!("approve"));
        // 无 plan 时 schema 只有 interaction/toolName。
        let without_plan = pending_user_input_backed_permission_to_elicitation_event(
            "t",
            &json!({"toolName": "ExitPlanMode", "input": {"plan": "   "}}),
        )
        .expect("elicitation");
        assert!(without_plan["schema"].get("plan").is_none());
    }
}
