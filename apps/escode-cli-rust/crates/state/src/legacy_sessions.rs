use super::legacy_attachments::file_content;
use super::legacy_attempt::check;
use super::legacy_projection::items;
use super::storage_write::{SessionWrite, write};
use crate::domain::session::Session;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use tokio_util::sync::CancellationToken;

pub(super) fn project(
    dest: &Connection,
    snapshot: &Connection,
    workspace: &str,
    cwd: &str,
    dir: &Path,
    artifacts: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut query=snapshot.prepare("SELECT id,title,title_source,time_created,time_updated,parent_id,revert,time_archived,task_type,trace_id,COALESCE(NULLIF(path,''),directory),directory FROM session WHERE COALESCE(NULLIF(TRIM(workspace_id),''),directory)=?1 ORDER BY time_created,id")?;
    let sessions = query
        .query_map([workspace], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, u64>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<u64>>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (
        id,
        title,
        title_source,
        created,
        updated,
        parent,
        revert,
        archived,
        task_type,
        trace,
        path,
        directory,
    ) in sessions
    {
        check(cancel)?;
        let exists: bool = dest.query_row(
            "SELECT EXISTS(SELECT 1 FROM rust_session WHERE workspace=?1 AND id=?2)",
            params![workspace, id],
            |r| r.get(0),
        )?;
        if exists {
            continue;
        }
        let entries = items(
            snapshot,
            "SELECT type,data FROM session_entry WHERE session_id=?1 ORDER BY time_created,id",
            &id,
        )?;
        let selection = entries
            .iter()
            .rev()
            .find(|(kind, _)| kind == "runtime/model_selection")
            .map(|(_, v)| &v["modelSelection"]);
        let mut session = Session::new(
            id.clone(),
            workspace.into(),
            selection
                .and_then(|s| s["providerId"].as_str())
                .unwrap_or("")
                .into(),
            selection
                .and_then(|s| s["modelId"].as_str())
                .unwrap_or("")
                .into(),
            selection
                .and_then(|s| s["options"]["reasoningLevel"].as_str())
                .unwrap_or("")
                .into(),
            super::id(),
            created,
        );
        session.workspace_path = Some(path);
        session.workspace_directory = Some(directory);
        session.trace_id = trace;
        session.parent_id = parent;
        session.task_type = task_type.clone();
        session.archived_at = archived;
        session.listed = matches!(
            task_type.as_str(),
            "interactive" | "workflow_parent" | "fork"
        );
        session.archived = archived.is_some();
        session.title = title;
        session.title_source = match title_source.as_deref() {
            Some("custom") => "custom",
            Some("generated") => "generated",
            Some("first_input") => "first_input",
            _ => "default",
        }
        .into();
        session.updated_at = updated;
        session.mode = entries
            .iter()
            .rev()
            .find(|(k, _)| k == "runtime/execution_state")
            .and_then(|(_, v)| v["mode"].as_str())
            .unwrap_or("build")
            .into();
        session.plan_enabled = entries
            .iter()
            .rev()
            .find(|(k, _)| k == "runtime/execution_state")
            .is_some_and(|(_, v)| v["planEnabled"] == true || v["mode"] == "plan");
        (session.todos, session.todos_updated_at) =
            super::legacy_todos::read(snapshot, &id, cancel)?;
        session.phase = "completedSuccess".into();
        let messages = items(
            snapshot,
            "SELECT id,data FROM message WHERE session_id=?1 ORDER BY sequence,time_created,id",
            &id,
        )?;
        let mut turn = String::new();
        let mut turn_header: Option<usize> = None;
        let mut turn_started_at = 0u64;
        let mut turn_ended_at = 0u64;
        let mut turn_rounds = 0u64;
        let mut positions: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        // 拥有持久 transcript message 身份的行（Node messageIdByRowId），冷导入 actions 只锚定这些行。
        let mut message_rows: Vec<usize> = Vec::new();
        // pending/running 工具行由 TurnComplete(cancelled) 统一收口（Node closeOpenToolRows）。
        let mut open_tools: Vec<usize> = Vec::new();
        // Node 把 timeline model_change 消息的标记推迟到下一个 TurnStarted。
        let mut pending_model: Option<(String, Value)> = None;
        let mut compact_boundary = Value::Null;
        let mut legacy_tail: Option<String> = None;
        let mut interrupted = false;
        let revert: Value = revert
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or(Value::Null);
        // 修复：原先按旧式 revert（messageID/partID）截断，真实数据的 conversation_rewind 记录
        // messageID 可指向首条消息，导致整段历史被丢弃。对齐 TS selectActiveConversationBranch：
        // 活跃分支 = keptMessageIDs + branchCutAfterMessageID 之后追加的消息；无 targetMessageID 不过滤。
        let ids: Vec<String> = messages.iter().map(|(id, _)| id.clone()).collect();
        let mut slots: Vec<Option<(String, Value)>> = messages.into_iter().map(Some).collect();
        let messages: Vec<(String, Value)> =
            crate::domain::rewind_branch::active_branch(&ids, &revert)
                .into_iter()
                .filter_map(|i| slots[i].take())
                .collect();
        for (mid, message) in messages {
            check(cancel)?;
            let parts_raw = super::legacy_projection::items_raw(
                snapshot,
                "SELECT id,data FROM part WHERE message_id=?1 ORDER BY sequence,time_created,id",
                &mid,
            )?;
            let input_texts = parts_raw
                .iter()
                .filter(|(_, part, _)| part["type"] == "tool")
                .map(|(id, _, data)| (id.clone(), super::legacy_message::tool_input_text(data)))
                .collect::<BTreeMap<_, _>>();
            let parts: Vec<(String, Value)> = parts_raw
                .into_iter()
                .map(|(id, part, _)| (id, part))
                .collect();
            super::legacy_projection::validate_parts(&parts)?;
            if super::legacy_topic_context::project(&mut session, &mid, &message, &parts) {
                continue;
            }
            for (_, part) in &parts {
                check(cancel)?;
                if part["type"] == "file"
                    && let Some(asset) =
                        super::legacy_attachments::snapshot(part, cwd, artifacts, dir)?
                {
                    session.attachments.insert(
                        part["url"]
                            .as_str()
                            .context("Missing attachment URL")?
                            .into(),
                        asset,
                    );
                }
            }
            let start = session.messages.len();
            for (_, p) in &parts {
                if p["compactBoundary"].is_object() {
                    compact_boundary = p["compactBoundary"].clone();
                }
                if let Some(t) = p["tail_start_id"].as_str() {
                    legacy_tail = Some(t.into());
                }
            }
            let role = message["role"]
                .as_str()
                .context("Invalid legacy message role")?;
            let now = message["time"]["created"].as_u64().unwrap_or(created);
            let text = parts
                .iter()
                .filter(|(_, p)| p["type"] == "text" && p["ignored"] != true)
                .filter_map(|(_, p)| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if role == "user" {
                let visible = message["visibility"] != "model-only"
                    && !matches!(
                        message["semantics"]["uiVisibility"].as_str(),
                        Some("hidden" | "debug")
                    );
                if visible || turn.is_empty() {
                    // 修复：原先每轮都调用 attach_pending_rows 全量扫描 rows。turn 一旦确定就不会再变空，
                    // 只有首轮之前产生的行可能缺 turnId；逐轮扫描会随历史增长退化为 O(rows²)。
                    let first_turn = turn.is_empty();
                    super::legacy_projection::finalize_turn(
                        &mut session,
                        turn_header,
                        turn_started_at,
                        turn_ended_at,
                        turn_rounds,
                        &mut open_tools,
                    );
                    turn = mid.clone();
                    // 首轮之前的边界标记（如 ∅→X 模型标记）在 Node 中属于它之后的第一轮。
                    if first_turn {
                        super::legacy_projection::attach_pending_rows(&mut session, &turn);
                    }
                    if let Some((pid, part)) = pending_model.take() {
                        super::legacy_projection::model_marker(
                            &mut session,
                            &turn,
                            &pid,
                            &part,
                            now,
                        );
                    }
                    turn_header = Some(super::legacy_projection::imported_turn_header(
                        &mut session,
                        &turn,
                        now,
                    ));
                    turn_started_at = now;
                    turn_ended_at = now;
                    turn_rounds = 0;
                }
                let mut user_row = None;
                if visible {
                    user_row = Some(super::legacy_projection::imported_user_input(
                        &mut session,
                        &turn,
                        &mid,
                        now,
                        &message,
                        &parts,
                        &text,
                    ));
                }
                let mut content = vec![];
                if !text.is_empty() {
                    content.push(json!({"type":"text","text":text}));
                }
                for (_, part) in &parts {
                    if part["type"] == "agent" {
                        content.push(json!({"type":"text","text":format!("[Selected agent: {}]",part["name"].as_str().unwrap_or(""))}));
                    }
                    if part["type"] == "file" {
                        content.extend(file_content(part, cwd, artifacts)?);
                    }
                }
                let content = if content.len() == 1 && content[0]["type"] == "text" {
                    content[0]["text"].clone()
                } else {
                    Value::Array(content)
                };
                if !content.as_array().is_some_and(Vec::is_empty)
                    && message["semantics"]["providerVisibility"] != "hidden"
                {
                    session
                        .messages
                        .push(json!({"role":"user","content":content}));
                    message_rows.extend(user_row);
                }
                if session.provider.is_empty()
                    && let Some(s) = message.get("modelSelection")
                {
                    session.provider = s["providerId"].as_str().unwrap_or("").into();
                    session.model = s["modelId"].as_str().unwrap_or("").into();
                    session.reasoning_level =
                        s["options"]["reasoningLevel"].as_str().unwrap_or("").into();
                }
            } else {
                ensure!(role == "assistant", "Unsupported legacy message role");
                if let Some(model_change) = super::legacy_projection::model_change_timeline(&parts)
                {
                    pending_model = Some(model_change);
                    continue;
                }
                super::legacy_message::Assistant {
                    mid: &mid,
                    message: &message,
                    parts: &parts,
                    text: &text,
                    now,
                    turn: &mut turn,
                    interrupted: &mut interrupted,
                    rounds: &mut turn_rounds,
                    ended_at: &mut turn_ended_at,
                    message_rows: &mut message_rows,
                    open_tools: &mut open_tools,
                    input_texts: &input_texts,
                    compact_boundary: &compact_boundary,
                    legacy_tail: legacy_tail.as_deref(),
                    positions: &positions,
                }
                .project(&mut session);
            }
            super::legacy_projection::timeline(&mut session, &parts, &turn, now);
            positions.insert(mid, (start, session.messages.len()));
        }
        super::legacy_projection::ledger(snapshot, &id, &mut session)?;
        super::legacy_projection::finalize_turn(
            &mut session,
            turn_header,
            turn_started_at,
            turn_ended_at,
            turn_rounds,
            &mut open_tools,
        );
        if interrupted {
            super::legacy_projection::mark_interrupted(&mut session, &turn);
        }
        super::legacy_row_post::imported_sequences(&mut session);
        super::legacy_row_post::imported_actions(&mut session, &message_rows);
        // 所有原始字段、快照和未实现投影仍完整保留在只读备份，import 的模型事实只追加一次。
        check(cancel)?;
        write(dest, workspace, Some(SessionWrite::new(&session)?), None)?;
    }
    Ok(())
}
