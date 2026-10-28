//! 「run 还欠着决定、主代理闲着」的提醒钟（docs/specs/rust-tool-search.md 同源的
//! docs/dynamic-workflow/transcript-and-notifications.md「Reminding an idle main agent」，对齐上游
//! `runtime-task/workflow-blocked-reminder.ts`）。
//!
//! 纯状态机：时钟、定时器、在飞 run 与开集都由 Engine 每 tick 递进来。规则：
//! - 量的是**会话空闲时长**，任何一轮开始即归零；
//! - 每个 run 最多提醒两次：第一次空闲 15 分钟，第二次再空闲 30 分钟；
//! - 用户发话、或该 run 的开集变化，重新给一对提醒；
//! - 入队即计数，先记后发。

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MINUTE_MS: u64 = 60_000;
pub const WORKFLOW_BLOCKED_REMINDER_DELAYS_MS: [u64; 2] = [15 * MINUTE_MS, 30 * MINUTE_MS];
const MAX_LISTED: usize = 8;
const MAX_TEXT_CHARS: usize = 2_000;
const MAX_NOTIFICATION_CHARS: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowOpenDecisions {
    pub questions: Vec<Value>,
    pub holes: Vec<Value>,
}

impl WorkflowOpenDecisions {
    pub fn is_empty(&self) -> bool {
        self.questions.is_empty() && self.holes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedReminder {
    pub run_id: String,
    pub open: WorkflowOpenDecisions,
    pub ordinal: usize,
    pub total: usize,
    pub now: u64,
}

/// 一个会话的提醒钟状态。Engine 按会话持有。
#[derive(Debug, Default, Clone)]
pub struct WorkflowBlockedClock {
    idle_since: Option<u64>,
    /// 每个 run 已提醒次数（0 即缺席）。唯一跨空闲段留存的计数。
    sent: BTreeMap<String, u8>,
    /// 上次看到的开集签名；变化即重新给一对提醒。
    signatures: BTreeMap<String, String>,
}

impl WorkflowBlockedClock {
    /// 任何一轮开始：钟停，空闲时长作废。
    pub fn session_busy(&mut self) {
        self.idle_since = None;
    }

    /// 命令队列排空。已经在空闲段里时是 no-op，不能把空闲时长冲回零。
    pub fn session_idle(&mut self, now: u64) {
        if self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
    }

    /// 用户发来消息：每个 run 重新得到一对提醒。
    pub fn user_spoke(&mut self) {
        self.sent.clear();
    }

    /// 这个 run 的开集变了：重新得到一对提醒；空闲钟从此刻重起。
    pub fn open_set_changed(&mut self, run_id: &str, now: u64) {
        self.sent.remove(run_id);
        self.signatures.remove(run_id);
        if self.idle_since.is_some() {
            self.idle_since = Some(now);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.idle_since.is_some()
    }

    /// 到点检查：返回本轮该发的提醒；发出后空闲段结束（提醒会唤起一轮）。
    pub fn due(
        &mut self,
        now: u64,
        live: &[(String, WorkflowOpenDecisions)],
    ) -> Vec<BlockedReminder> {
        let live_ids: BTreeSet<&str> = live.iter().map(|(run_id, _)| run_id.as_str()).collect();
        self.sent
            .retain(|run_id, _| live_ids.contains(run_id.as_str()));
        self.signatures
            .retain(|run_id, _| live_ids.contains(run_id.as_str()));

        for (run_id, open) in live {
            let signature = signature(open);
            if self.signatures.get(run_id) != Some(&signature) {
                let seen_before = self.signatures.insert(run_id.clone(), signature).is_some();
                self.sent.remove(run_id);
                // 首次观察不是「开集变化」：不能把刚开始的空闲段冲回零。
                if seen_before && self.idle_since.is_some() {
                    self.idle_since = Some(now);
                }
            }
        }

        let Some(idle_since) = self.idle_since else {
            return Vec::new();
        };
        let idle_ms = now.saturating_sub(idle_since);
        let mut due = Vec::new();
        for (run_id, open) in live {
            if open.is_empty() {
                continue;
            }
            let sent = usize::from(*self.sent.get(run_id).unwrap_or(&0));
            let Some(delay) = WORKFLOW_BLOCKED_REMINDER_DELAYS_MS.get(sent) else {
                continue;
            };
            if idle_ms < *delay {
                continue;
            }
            self.sent.insert(run_id.clone(), (sent + 1) as u8);
            due.push(BlockedReminder {
                run_id: run_id.clone(),
                open: open.clone(),
                ordinal: sent + 1,
                total: WORKFLOW_BLOCKED_REMINDER_DELAYS_MS.len(),
                now,
            });
        }
        if !due.is_empty() {
            // 提醒会同步唤起一轮；即使入口把通知丢了，也按「已尝试」结束本空闲段，
            // 避免同一个 tick 重复入队。
            self.idle_since = None;
        }
        due
    }
}

fn signature(open: &WorkflowOpenDecisions) -> String {
    let mut parts: Vec<String> = open
        .questions
        .iter()
        .filter_map(|question| question["qid"].as_str().map(str::to_owned))
        .chain(
            open.holes
                .iter()
                .filter_map(|hole| hole["siteId"].as_str().map(str::to_owned)),
        )
        .collect();
    parts.sort();
    parts.join("\u{0}")
}

/// `originMeta.workflowBlocked`：只带计数与最久等待，原文在各自的通知行上。
pub fn blocked_meta(open: &WorkflowOpenDecisions, now: u64) -> Value {
    let oldest = open
        .questions
        .iter()
        .filter_map(|question| question["askedAt"].as_u64())
        .chain(open.holes.iter().filter_map(|hole| hole["since"].as_u64()))
        .map(|since| now.saturating_sub(since))
        .max()
        .unwrap_or(0);
    json!({
        "questions": open.questions.len(),
        "holes": open.holes.len(),
        "sinceMs": oldest,
    })
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn bounded_text(value: &str) -> String {
    if value.chars().count() <= MAX_TEXT_CHARS {
        return value.to_owned();
    }
    let mut out: String = value.chars().take(MAX_TEXT_CHARS - 1).collect();
    out.push('…');
    out
}

fn waited_ms(now: u64, since: Option<u64>) -> u64 {
    since.map_or(0, |since| now.saturating_sub(since))
}

fn count_noun(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// 上游 `formatWorkflowBlockedNotification` 的等价文案。
pub fn format_blocked_notification(
    run_label: &str,
    reminder: &BlockedReminder,
    draft_path: Option<&str>,
) -> String {
    let now = reminder.now;
    let questions = &reminder.open.questions;
    let holes = &reminder.open.holes;
    let mut lines = vec![
        "[SYSTEM NOTIFICATION - NOT USER INPUT]".to_owned(),
        "This is an automated workflow event, NOT a message from the user.".to_owned(),
        "Do NOT interpret this as user acknowledgement, confirmation, or response to any pending question.".to_owned(),
        String::new(),
        "<workflow-blocked>".to_owned(),
        format!("  <run-id>{}</run-id>", escape_xml(&reminder.run_id)),
        format!("  <run>{}</run>", escape_xml(run_label)),
        format!("  <reminder>{} of {}</reminder>", reminder.ordinal, reminder.total),
    ];
    let shown_questions = questions.len().min(MAX_LISTED);
    if !questions.is_empty() {
        lines.push(format!(
            "  <open-questions count=\"{}\" shown=\"{}\">",
            questions.len(),
            shown_questions
        ));
        for question in questions.iter().take(MAX_LISTED) {
            let subagent = question["actorName"]
                .as_str()
                .or_else(|| question["actor"].as_str())
                .unwrap_or_default();
            lines.push(format!(
                "    <question id=\"{}\" subagent=\"{}\" waited-ms=\"{}\">",
                escape_xml(question["qid"].as_str().unwrap_or_default()),
                escape_xml(subagent),
                waited_ms(now, question["askedAt"].as_u64())
            ));
            lines.push(format!(
                "      <text>{}</text>",
                escape_xml(&bounded_text(
                    question["question"].as_str().unwrap_or_default()
                ))
            ));
            if let Some(context) = question["context"]
                .as_str()
                .filter(|value| !value.is_empty())
            {
                lines.push(format!(
                    "      <context>{}</context>",
                    escape_xml(&bounded_text(context))
                ));
            }
            lines.push("    </question>".to_owned());
        }
        lines.push("  </open-questions>".to_owned());
    }
    let shown_holes = holes.len().min(MAX_LISTED);
    if !holes.is_empty() {
        lines.push(format!(
            "  <open-holes count=\"{}\" shown=\"{}\">",
            holes.len(),
            shown_holes
        ));
        for hole in holes.iter().take(MAX_LISTED) {
            let line = hole["line"]
                .as_u64()
                .map(|line| format!(" line=\"{line}\""))
                .unwrap_or_default();
            lines.push(format!(
                "    <hole id=\"{}\" name=\"{}\" type=\"{}\"{} waited-ms=\"{}\"/>",
                escape_xml(hole["siteId"].as_str().unwrap_or_default()),
                escape_xml(hole["name"].as_str().unwrap_or_default()),
                escape_xml(hole["type"].as_str().unwrap_or_default()),
                line,
                waited_ms(now, hole["since"].as_u64())
            ));
        }
        lines.push("  </open-holes>".to_owned());
    }
    if let Some(draft_path) = draft_path {
        lines.push(format!("  <draft>{}</draft>", escape_xml(draft_path)));
    }
    lines.push("</workflow-blocked>".to_owned());
    lines.push(String::new());

    let total = questions.len() + holes.len();
    let oldest = questions
        .iter()
        .filter_map(|question| question["askedAt"].as_u64())
        .chain(holes.iter().filter_map(|hole| hole["since"].as_u64()))
        .map(|since| now.saturating_sub(since))
        .max()
        .unwrap_or(0);
    let minutes = (oldest / MINUTE_MS).max(1);
    let what = [
        (!questions.is_empty()).then(|| count_noun(questions.len(), "question")),
        (!holes.is_empty()).then(|| count_noun(holes.len(), "hole")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" and ");
    if total == 1 {
        lines.push(format!(
            "Workflow run {run_label} ({}) is still waiting on you: {what} is open, for {minutes} minutes, and you have not acted on it since your last turn ended.",
            reminder.run_id
        ));
    } else {
        lines.push(format!(
            "Workflow run {run_label} ({}) is still waiting on you: {what} are open, the oldest for {minutes} minutes, and you have not acted on them since your last turn ended.",
            reminder.run_id
        ));
    }
    if !questions.is_empty() {
        lines.push("Next step for each question: call ResolveWorkflowQuestion with its id as question_id and your answer.".to_owned());
    }
    if !holes.is_empty() {
        let context = draft_path.map_or_else(
            || {
                format!(
                    "read the run's script with GetWorkflowRun (run_id=\"{}\")",
                    reminder.run_id
                )
            },
            |path| format!("read the draft at {path}"),
        );
        lines.push(format!(
            "Next step for each hole: {context} for the context, write only the statements of the hole's function returning its type, and call FillWorkflowHole with run_id=\"{}\" and its id as hole_id and the body as `script`.",
            reminder.run_id
        ));
    }
    if shown_questions < questions.len() || shown_holes < holes.len() {
        lines.push(format!(
            "GetWorkflowRun lists the ones not shown here (run_id=\"{}\").",
            reminder.run_id
        ));
    }
    lines.push("If a decision needs the user, tell the user exactly what you need from them and end your turn; do not guess an answer to make the reminders stop.".to_owned());
    lines.push(if reminder.ordinal < reminder.total {
        "One more reminder follows if nothing changes.".to_owned()
    } else {
        "This is the last reminder: nothing further is sent until the user writes or this run's open decisions change.".to_owned()
    });
    lines.push(
        "Either way the parked branches keep waiting; nothing times out on their behalf."
            .to_owned(),
    );

    let joined = lines.join("\n");
    if joined.chars().count() <= MAX_NOTIFICATION_CHARS {
        joined
    } else {
        format!(
            "{}…",
            joined
                .chars()
                .take(MAX_NOTIFICATION_CHARS - 1)
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(qid: &str, asked_at: u64) -> Value {
        json!({"qid": qid, "actor": "sub", "question": "which?", "askedAt": asked_at})
    }

    fn hole(site_id: &str, since: u64) -> Value {
        json!({"siteId": site_id, "name": "h", "type": "Plan", "state": "waiting", "since": since})
    }

    fn open(questions: Vec<Value>, holes: Vec<Value>) -> WorkflowOpenDecisions {
        WorkflowOpenDecisions { questions, holes }
    }

    #[test]
    fn reminds_at_fifteen_then_thirty_minutes_idle() {
        let mut clock = WorkflowBlockedClock::default();
        let live = vec![("run-1".to_string(), open(vec![question("q1", 0)], vec![]))];
        clock.session_idle(1_000);
        assert!(clock.due(1_000 + 15 * MINUTE_MS - 1, &live).is_empty());
        let first = clock.due(1_000 + 15 * MINUTE_MS, &live);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].ordinal, 1);
        // 提醒唤起一轮：重新空闲后从第 2 次（30 分钟）算。
        clock.session_idle(2_000_000);
        assert!(clock.due(2_000_000 + 30 * MINUTE_MS - 1, &live).is_empty());
        let second = clock.due(2_000_000 + 30 * MINUTE_MS, &live);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].ordinal, 2);
        clock.session_idle(3_000_000);
        assert!(clock.due(3_000_000 + 60 * MINUTE_MS, &live).is_empty());
    }

    #[test]
    fn user_spoke_and_open_set_change_reset_the_pair() {
        let mut clock = WorkflowBlockedClock::default();
        let live = vec![("run-1".to_string(), open(vec![question("q1", 0)], vec![]))];
        clock.session_idle(0);
        assert_eq!(clock.due(15 * MINUTE_MS, &live).len(), 1);
        clock.user_spoke();
        clock.session_idle(60 * MINUTE_MS);
        let again = clock.due(60 * MINUTE_MS + 15 * MINUTE_MS, &live);
        assert_eq!(again[0].ordinal, 1);

        let changed = vec![(
            "run-1".to_string(),
            open(vec![question("q1", 0)], vec![hole("h1", 0)]),
        )];
        clock.session_idle(200 * MINUTE_MS);
        // 开集变化在这一次 tick 被发现，空闲钟从此刻重起；15 分钟后才提醒。
        assert!(clock.due(200 * MINUTE_MS, &changed).is_empty());
        let after_change = clock.due(200 * MINUTE_MS + 15 * MINUTE_MS, &changed);
        assert_eq!(after_change[0].ordinal, 1);
    }

    #[test]
    fn busy_and_no_open_decisions_do_not_remind() {
        let mut clock = WorkflowBlockedClock::default();
        let live = vec![("run-1".to_string(), open(vec![], vec![]))];
        clock.session_idle(0);
        assert!(clock.due(60 * MINUTE_MS, &live).is_empty());
        clock.session_busy();
        let live = vec![("run-1".to_string(), open(vec![question("q1", 0)], vec![]))];
        assert!(clock.due(60 * MINUTE_MS, &live).is_empty());
    }

    #[test]
    fn formatter_matches_upstream_shape() {
        let reminder = BlockedReminder {
            run_id: "run-1".to_string(),
            open: open(vec![question("q1", 0)], vec![hole("h1", 0)]),
            ordinal: 1,
            total: 2,
            now: 10 * MINUTE_MS,
        };
        let text = format_blocked_notification("demo", &reminder, Some("/tmp/draft.ts"));
        assert!(text.contains("<workflow-blocked>"));
        assert!(text.contains("<open-questions count=\"1\" shown=\"1\">"));
        assert!(text.contains("<open-holes count=\"1\" shown=\"1\">"));
        assert!(text.contains("<draft>/tmp/draft.ts</draft>"));
        assert!(text.contains("One more reminder follows"));
        assert!(text.contains("10 minutes"));
    }

    #[test]
    fn blocked_meta_counts_and_oldest_wait() {
        let meta = blocked_meta(
            &open(vec![question("q1", 1_000)], vec![hole("h1", 4_000)]),
            10_000,
        );
        assert_eq!(meta, json!({"questions":1,"holes":1,"sinceMs":9_000}));
    }
}
