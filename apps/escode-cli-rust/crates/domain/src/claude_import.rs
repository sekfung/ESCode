//! Claude Code 历史导入（TS `escodeSessionImportHistorySchema` claudeCode 分支 +
//! `persistImportedSessionHistory`；docs/specs/rust-legacy-session-methods.md）：
//! 解析 `session/create` 导入参数，并把消息数组投影为行 + canonical 消息，与 legacy 导入器同形。
use super::{
    session::Session,
    session_listing::{ListParams, WorkspaceRef},
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeHistory {
    pub source: String,
    pub title: Option<String>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub messages: Vec<ClaudeMessage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Import {
    pub session_id: Option<String>,
    pub workspace: WorkspaceRef,
    pub parent_session_id: Option<String>,
    pub mode: Option<String>,
    pub model: Option<Value>,
    pub persistence: Option<String>,
    pub thought_level: Option<String>,
    pub title_generation_enabled: Option<bool>,
    pub mcp_servers: Option<Vec<Value>>,
    pub tool_allowlist: Option<Vec<String>>,
    pub tool_denylist: Option<Vec<String>>,
    pub imported_history: ClaudeHistory,
    pub dynamic_workflow_enabled: Option<bool>,
}

impl Import {
    pub fn parse(value: &Value) -> Result<Self> {
        let input = Self::parse_common(value)?;
        let history = &input.imported_history;
        ensure!(
            history.source == "claudeCode",
            "Unsupported imported history source"
        );
        ensure!(!history.messages.is_empty(), "Imported history is empty");
        ensure!(
            history
                .messages
                .iter()
                .all(|m| matches!(m.role.as_str(), "user" | "assistant")),
            "Invalid imported message role"
        );
        Ok(input)
    }

    /// 外层校验：null 字段、workspace、tool profile、mode/persistence 取值。
    fn parse_common(value: &Value) -> Result<Self> {
        non_null(value)?;
        non_null(&value["importedHistory"])?;
        let mut input: Self = serde_json::from_value(value.clone())?;
        input.workspace = ListParams::parse(&json!({"workspace":value["workspace"]}))?
            .workspace
            .ok_or_else(|| anyhow::anyhow!("Workspace required"))?;
        for s in input
            .session_id
            .iter_mut()
            .chain(input.parent_session_id.iter_mut())
            .chain(input.thought_level.iter_mut())
        {
            nonempty(s)?;
        }
        ensure!(
            input
                .mode
                .as_deref()
                .is_none_or(|m| matches!(m, "build" | "yolo" | "plan" | "auto")),
            "Invalid imported mode"
        );
        ensure!(
            input
                .persistence
                .as_deref()
                .is_none_or(|p| matches!(p, "immediate" | "deferred")),
            "Invalid persistence"
        );
        ensure!(
            input.tool_allowlist.as_ref().is_none_or(Vec::is_empty)
                && input.tool_denylist.as_ref().is_none_or(Vec::is_empty)
                && input.dynamic_workflow_enabled != Some(true),
            "Unsupported imported tool profile"
        );
        // 标题来源由 importedHistory 拥有，不启动异步标题生成。
        let _ = input.title_generation_enabled;
        Ok(input)
    }
}

fn non_null(object: &Value) -> Result<()> {
    ensure!(
        object
            .as_object()
            .is_some_and(|m| m.values().all(|v| !v.is_null())),
        "Null import field"
    );
    Ok(())
}

fn nonempty(value: &mut String) -> Result<()> {
    *value = value.trim().to_owned();
    ensure!(!value.is_empty(), "Empty imported identity");
    Ok(())
}

/// 会话时间：`createdAt ?? 首条时间`，`updatedAt` 不早于 `createdAt`（TS 同）。
pub fn times(history: &ClaudeHistory, now: u64) -> (u64, u64) {
    let created = history
        .created_at
        .or_else(|| history.messages.first().and_then(|m| m.timestamp))
        .unwrap_or(now);
    let updated = history
        .updated_at
        .or_else(|| history.messages.last().and_then(|m| m.timestamp))
        .unwrap_or(created);
    (created, updated.max(created))
}

pub fn build(session: &mut Session, history: &ClaudeHistory, created: u64) {
    let mut turn = String::new();
    // TS 以数组顺序为准，写库前把时间收敛为严格递增，避免 assistant 排到 user 之前。
    let mut last = created.saturating_sub(1);
    for (index, message) in history.messages.iter().enumerate() {
        let raw = message.timestamp.unwrap_or(created + index as u64);
        let at = if raw > last { raw } else { last + 1 };
        last = at;
        let entity = format!("{}_import_{index}", session.id);
        let user = message.role == "user";
        if user || turn.is_empty() {
            turn = entity.clone();
            let mut header = session.row("turnHeader", &turn, &turn, at);
            header["origin"] = "userInput".into();
            header["state"] = "completedSuccess".into();
            header["startedAt"] = at.into();
            header["endedAt"] = at.into();
            session.rows.push(header);
        }
        if user {
            let mut row = session.row("userInput", &turn, &entity, at);
            row["text"] = message.content.clone().into();
            row["origin"] = "realUser".into();
            session.rows.push(row);
            session.append_message(json!({"role":"user","content":message.content}));
        } else {
            let mut row = session.row("assistantText", &turn, &format!("{entity}_text"), at);
            row["text"] = message.content.clone().into();
            row["assistantResponseId"] = entity.clone().into();
            row["state"] = "complete".into();
            session.rows.push(row);
            session.append_message(json!({"role":"assistant","content":message.content,
                "_escode_origin":{"provider":"unknown","model":"unknown"}}));
        }
    }
    session.updated_at = session.updated_at.max(last);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(messages: &[(&str, &str, Option<u64>)]) -> ClaudeHistory {
        ClaudeHistory {
            source: "claudeCode".into(),
            title: None,
            created_at: None,
            updated_at: None,
            messages: messages
                .iter()
                .map(|(role, content, timestamp)| ClaudeMessage {
                    role: (*role).into(),
                    content: (*content).into(),
                    timestamp: *timestamp,
                })
                .collect(),
        }
    }

    #[test]
    fn messages_become_turns_rows_and_canonical_history_in_order() {
        // assistant 时间早于 user：按数组顺序收敛为递增。
        let h = history(&[
            ("user", "q1", Some(200)),
            ("assistant", "a1", Some(100)),
            ("user", "q2", None),
        ]);
        let (created, updated) = times(&h, 9);
        assert_eq!((created, updated), (200, 200));
        let mut s = Session::new(
            "claude-import-x".into(),
            "w".into(),
            "".into(),
            "".into(),
            "".into(),
            "t".into(),
            created,
        );
        build(&mut s, &h, created);
        let kinds: Vec<_> = s.rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "turnHeader",
                "userInput",
                "assistantText",
                "turnHeader",
                "userInput"
            ]
        );
        let times: Vec<_> = s
            .rows
            .iter()
            .map(|r| r["createdAt"].as_u64().unwrap())
            .collect();
        assert_eq!(times, [200, 200, 201, 202, 202]);
        assert_eq!(s.rows[2]["turnId"], s.rows[1]["turnId"]);
        let roles: Vec<_> = s
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
        assert_eq!(s.messages[1]["content"], "a1");
    }

    #[test]
    fn leading_assistant_gets_its_own_turn_header() {
        let h = history(&[("assistant", "hello", Some(5))]);
        let mut s = Session::new(
            "s".into(),
            "w".into(),
            "".into(),
            "".into(),
            "".into(),
            "t".into(),
            5,
        );
        build(&mut s, &h, 5);
        assert_eq!(s.rows[0]["kind"], "turnHeader");
        assert_eq!(s.rows[1]["kind"], "assistantText");
    }
}
