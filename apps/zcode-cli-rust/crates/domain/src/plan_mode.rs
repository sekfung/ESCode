//! plan 模式的纯规则：模型可见文案、模式 reminder 节奏、plan 文件名、审批载荷与应答归一。
//! 与 TS 对齐的依据见 docs/specs/rust-plan-mode.md；reminder 文案由生成器从 TS 导出。
use serde_json::{Value, json};
use std::sync::LazyLock;

pub const EXIT_PLAN_MODE: &str = "ExitPlanMode";
pub const ENTER_PLAN_MODE: &str = "EnterPlanMode";
/// 模式 reminder 在历史中的来源标记（TS metadata.source = "runtime_mode"）。
pub const MODE_REMINDER_SOURCE: &str = "runtime_mode";
pub const EXIT_REMINDER_SOURCE: &str = "plan_mode_exit";
/// TS `EXIT_PLAN_DENIED_BY_USER_MESSAGE`。
pub const NOT_APPROVED: &str = "The plan was not approved by the user.";
/// TS exitPlanModeHandler 在非 plan 模式下的错误。
pub const NOT_IN_PLAN: &str = "You are not in plan mode. This tool is only for exiting plan mode after writing a plan. If your plan was already approved, continue with implementation.";
/// TS `EXIT_PLAN_MODE_APPROVAL_APPROVE`。
const APPROVE: &str = "approve";
/// TS RUNTIME_MODE_REMINDER_CONFIG。
const TURNS_BETWEEN_ATTACHMENTS: usize = 5;
const FULL_REMINDER_EVERY_N_ATTACHMENTS: usize = 5;

struct Reminders {
    full: String,
    sparse: String,
    exit: String,
}
static REMINDERS: LazyLock<Reminders> = LazyLock::new(|| {
    let v: Value = serde_json::from_str(include_str!("plan_mode_reminders.json"))
        .expect("validated plan mode reminders");
    let text = |k: &str| v[k].as_str().expect("plan reminder text").to_owned();
    Reminders {
        full: text("full"),
        sparse: text("sparse"),
        exit: text("exit"),
    }
});

pub fn wrap(body: &str) -> String {
    format!("<system-reminder>\n{body}\n</system-reminder>")
}

/// 是否算作一次真实用户轮次：无来源标记、且不是系统合成的 reminder / 通知。
fn is_real_user(m: &Value) -> bool {
    m["role"] == "user"
        && m.get("_zcode_source").is_none()
        && !m["content"].as_str().is_some_and(|c| {
            c.starts_with("<system-reminder>") || c.starts_with("<task-notification>")
        })
}

/// TS `buildRuntimeModeReminderBody`：plan 开启时决定是否在本次用户输入前插入模式 reminder。
/// `messages` 为插入前的会话历史（不含本次输入）。
pub fn mode_reminder(messages: &[Value], plan_enabled: bool) -> Option<Value> {
    if !plan_enabled {
        return None;
    }
    let mut human_turns = 0;
    let mut found = false;
    for m in messages.iter().rev() {
        if m["_zcode_source"] == MODE_REMINDER_SOURCE {
            found = true;
            break;
        }
        if is_real_user(m) {
            human_turns += 1;
        }
    }
    if found && human_turns < TURNS_BETWEEN_ATTACHMENTS {
        return None;
    }
    let next = messages
        .iter()
        .filter(|m| m["_zcode_source"] == MODE_REMINDER_SOURCE)
        .count()
        + 1;
    let body = if next % FULL_REMINDER_EVERY_N_ATTACHMENTS == 1 {
        &REMINDERS.full
    } else {
        &REMINDERS.sparse
    };
    Some(json!({"role":"user","content":wrap(body),"_zcode_source":MODE_REMINDER_SOURCE}))
}

pub fn exit_reminder() -> Value {
    json!({"role":"user","content":wrap(&REMINDERS.exit),"_zcode_source":EXIT_REMINDER_SOURCE})
}

/// TS `formatEnterPlanModeModelContent`。
pub fn enter_result() -> String {
    "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.

In plan mode, you should:
1. Thoroughly explore the codebase to understand existing patterns
2. Identify similar features and architectural approaches
3. Consider multiple approaches and their trade-offs
4. Use AskUserQuestion if you need to clarify the approach
5. Design a concrete implementation strategy
6. When ready, use ExitPlanMode to present your plan for approval

Remember: DO NOT write or edit any files yet. This is a read-only exploration and planning phase.".into()
}

/// TS `formatExitPlanModeModelContent`。
pub fn exit_result(plan: &str) -> String {
    let plan = plan.trim();
    if plan.is_empty() {
        return "User has approved exiting plan mode. You can now proceed.".into();
    }
    format!(
        "User has approved your plan. You can now start coding. Start with updating your todo list if applicable.\n\n## Approved Plan:\n{plan}"
    )
}

/// TS `sanitizePlanFileSessionId`：`plan-<id>.md`；清洗后为空返回 None。
pub fn plan_file_name(session_id: &str) -> Option<String> {
    let mut out = String::new();
    let mut in_run = false;
    for c in session_id.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    (!trimmed.is_empty()).then(|| format!("plan-{trimmed}.md"))
}

/// 审批交互载荷（TS product-projection 的 ExitPlanMode 分支）。`prompt` 为 ask 判定原因。
pub fn approval_payload(call_id: &str, input: &Value, prompt: &str) -> Value {
    json!({
        "kind": "userInput",
        "prompt": prompt,
        "freeText": true,
        "toolCallId": call_id,
        "toolName": EXIT_PLAN_MODE,
        "input": input,
        "schema": {"interaction": "plan_approval", "toolName": EXIT_PLAN_MODE},
        "questions": [{
            "question": prompt,
            "header": "Plan",
            "options": [{
                "value": APPROVE,
                "label": "Approve",
                "description": "Exit plan mode and start implementation.",
            }],
        }],
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Feedback(String),
    Decline,
}

/// TS `v4AnswerToPlanApprovalResponse` + `planApprovalResponseToBrokerResult`。
pub fn decide(answer: &Value) -> Decision {
    let accepted = |content: &Value| {
        let raw = content["answers"]["Review this implementation plan."]
            .as_str()
            .or_else(|| content["answer_0"].as_str())
            .or_else(|| content["answer"].as_str())
            .map(str::trim)
            .unwrap_or("");
        match raw {
            "" => Decision::Decline,
            APPROVE => Decision::Approve,
            text => Decision::Feedback(text.to_owned()),
        }
    };
    if let Some(action) = answer["action"].as_str() {
        return if action == "accept" {
            accepted(&answer["content"])
        } else {
            Decision::Decline
        };
    }
    if matches!(
        answer["optionId"].as_str(),
        Some("allowOnce" | "allowAlways")
    ) {
        return Decision::Approve;
    }
    match answer["freeText"].as_str().map(str::trim) {
        Some(text) if !text.is_empty() => accepted(&json!({"answer": text})),
        _ => Decision::Decline,
    }
}

#[cfg(test)]
#[path = "plan_mode_tests.rs"]
mod tests;
