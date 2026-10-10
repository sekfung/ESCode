use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub mcp_servers: Option<Vec<String>>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub memory: Option<String>,
    pub name: String,
    pub description: String,
    pub source: String,
    pub system_prompt: String,
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    #[serde(default)]
    pub max_turns: Option<usize>,
    #[serde(default)]
    pub model_selection: Option<Value>,
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub inject_agents_md: Option<bool>,
    /// TS `AgentProfile.permissionMode`：仅 `auto`/`plan`，且只由用户级/插件来源声明
    /// （项目级在解析时剥离）。见 docs/specs/rust-permission-modes.md。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
}
impl Profile {
    pub fn allows(&self, name: &str) -> bool {
        if name.starts_with("mcp__")
            && self.mcp_servers.as_ref().is_some_and(|servers| {
                !servers.iter().any(|s| {
                    name.starts_with(&format!(
                        "mcp__{}__",
                        s.chars()
                            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                                c
                            } else {
                                '_'
                            })
                            .collect::<String>()
                    ))
                })
            })
        {
            return false;
        }
        let name = if name == "Task" { "Agent" } else { name };
        self.tools
            .as_ref()
            .is_none_or(|tools| tools.iter().any(|t| matches(t, name)))
            && !self.disallowed_tools.iter().any(|t| matches(t, name))
    }
}
fn matches(rule: &str, name: &str) -> bool {
    rule == name || rule == "*" || rule.strip_suffix('*').is_some_and(|p| name.starts_with(p))
}
pub fn builtins() -> Vec<Profile> {
    serde_json::from_str(include_str!("agent_profiles.json")).expect("validated agent profiles")
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub child_id: String,
    pub parent_run: String,
    pub call_id: String,
    pub agent_type: String,
    pub description: String,
    pub prompt: String,
    pub status: String,
    pub background: bool,
    pub notified: bool,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub output: String,
    pub output_file: String,
    pub tool_uses: u64,
    pub tokens: u64,
}
impl Task {
    pub fn running(&self) -> bool {
        self.status == "running"
    }
    pub fn summary(&self) -> Value {
        let mut value = json!({"childSessionId":self.child_id,"agentId":self.id,"toolCallId":self.call_id,
            "subagentType":self.agent_type,"title":self.description,"status":match self.status.as_str(){"completed"=>"success", "interrupted"=>"lost",other=>other},"startedAt":self.started_at});
        if let Some(at) = self.ended_at {
            value["endedAt"] = at.into();
        }
        value
    }
    pub fn background_work(&self) -> Value {
        json!({"workId":self.id,"kind":"subagent","title":self.description,
        "status":if self.running(){"running"}else{"resultPending"},"startedAt":self.started_at,"cancellable":self.running(),"anchorRowId":null,"childSessionId":self.child_id})
    }
    pub fn content(&self) -> String {
        if self.running() {
            format!(
                "Async agent launched successfully.\nagentId: {} (internal ID - do not mention to user. Use SendMessage with to: '{}' to continue this agent.)\nThe agent is working in the background. You will be notified automatically when it completes.\noutput_file: {}\nDo NOT Read or tail this file via the shell tool.",
                self.id, self.id, self.output_file
            )
        } else {
            format!(
                "{}\nagentId: {} (use SendMessage with to: '{}' to continue this agent)\n<usage>subagent_tokens: {}\ntool_uses: {}\nduration_ms: {}</usage>",
                self.output,
                self.id,
                self.id,
                self.tokens,
                self.tool_uses,
                self.ended_at
                    .unwrap_or(self.started_at)
                    .saturating_sub(self.started_at)
            )
        }
    }
    /// TS formatLocalAgentTaskNotification（subagent/completion-notification.ts + runtime-task/notification.ts）。
    pub fn notification(&self) -> String {
        let status = match self.status.as_str() {
            "completed" => "completed",
            "failed" => "failed",
            _ => "stopped",
        };
        let mut summary = format!("Agent {} task \"{}\" {status}.", self.agent_type, self.description);
        if status == "failed" && !self.output.trim().is_empty() {
            summary = format!("{summary} {}", self.output);
        }
        let mut lines = vec![
            "<task-notification>".to_owned(),
            format!("<task-id>{}</task-id>", escape(&self.id)),
            format!("<tool-use-id>{}</tool-use-id>", escape(&self.call_id)),
        ];
        if !self.output_file.is_empty() {
            lines.push(format!("<output-file>{}</output-file>", escape(&self.output_file)));
        }
        lines.push(format!("<status>{status}</status>"));
        lines.push(format!("<summary>{}</summary>", escape(&summary)));
        let duration = self.ended_at.unwrap_or(self.started_at).saturating_sub(self.started_at);
        match status {
            "completed" => {
                lines.push(format!("<result>{}</result>", escape(&self.output)));
                lines.push(format!(
                    "<usage><subagent_tokens>{}</subagent_tokens><tool_uses>{}</tool_uses><duration_ms>{duration}</duration_ms></usage>",
                    self.tokens, self.tool_uses
                ));
            }
            "failed" => {
                lines.push(format!("<error>{}</error>", escape(&self.output)));
                lines.push(format!("<usage><duration_ms>{duration}</duration_ms></usage>"));
            }
            _ => lines.push(format!("<usage><duration_ms>{duration}</duration_ms></usage>")),
        }
        lines.push("</task-notification>".to_owned());
        lines.join("
")
    }
    pub fn task_output(&self, timed_out: bool) -> Value {
        json!({"retrieval_status":if self.running(){if timed_out{"timeout"}else{"not_ready"}}else{"success"},
        "task":{"task_id":self.id,"task_type":"local_agent","status":self.status,"description":self.description,"output":self.output,"prompt":self.prompt,"outputFile":self.output_file}})
    }
}
/// TS escapeXml。
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

pub fn projection(s: &crate::session::Session, patch: &mut Value) {
    if s.children.is_empty() {
        return;
    }
    patch["subagents"] = json!({"revision":s.revision,"childSessionIds":s.children.values().map(|t|&t.child_id).collect::<Vec<_>>(),
        "running":s.children.values().filter(|t|t.running()).map(|t|t.summary()).collect::<Vec<_>>(),"endedTotal":s.children.values().filter(|t|!t.running()).count()});
    patch["backgroundWorks"].as_array_mut().unwrap().extend(
        s.children
            .values()
            .filter(|t| t.background && (t.running() || !t.notified))
            .map(|t| t.background_work()),
    );
}
