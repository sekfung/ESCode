//! ReadSessionContext 的会话来源（docs/specs/rust-read-session-context.md）：先按 id 跨 workspace 查 Rust 库，
//! 找不到再只读查 TS 库（尚未导入的会话）。两者都转成 TS `MessageWithParts` 形态。

use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use zcode_cli_domain::session_context::{SessionInfo, message_from_ts_json, messages_from_rust};

pub(super) fn read(
    conn: &Connection,
    legacy: Option<&Path>,
    id: &str,
) -> Result<Option<zcode_cli_domain::session_context::SessionSource>> {
    let workspace: Option<String> = conn
        .query_row(
            "SELECT workspace FROM rust_session WHERE id=?1 LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(workspace) = workspace
        && let Some(session) = super::storage_read::load_session(conn, &workspace, id)?
    {
        let directory = session
            .workspace_directory
            .clone()
            .unwrap_or_else(|| session.workspace.clone());
        // Node 会话的 path 即 workspace 路径（与 directory 相同时也输出 Path 行）。
        let path = Some(
            session
                .workspace_path
                .clone()
                .unwrap_or_else(|| directory.clone()),
        );
        let messages = messages_from_rust(
            &session.messages,
            &session.rows,
            session.context.offset,
            session.context.summary.as_deref(),
            session.updated_at as i64,
        );
        let info = SessionInfo {
            id: session.id,
            title: session.title,
            directory,
            path,
        };
        return Ok(Some((info, messages)));
    }
    match legacy {
        Some(source) if source.exists() => legacy_session(source, id),
        _ => Ok(None),
    }
}

/// TS `getSession` + `messages()`：只读打开，按 TS 的排序取消息与 part，拼成 JSON 文本以保留工具 input 的键顺序。
fn legacy_session(
    source: &Path,
    id: &str,
) -> Result<Option<zcode_cli_domain::session_context::SessionSource>> {
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let info = conn
        .query_row(
            "SELECT id,title,directory,path FROM session WHERE id=?1",
            [id],
            |row| {
                Ok(SessionInfo {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    directory: row.get(2)?,
                    path: row.get(3)?,
                })
            },
        )
        .optional()?;
    let Some(info) = info else {
        return Ok(None);
    };
    let mut parts: Vec<(String, String, String)> = Vec::new();
    {
        let mut query = conn.prepare(
            "SELECT message_id,id,data FROM part WHERE session_id=?1 ORDER BY message_id, sequence IS NULL, sequence, time_created, id",
        )?;
        let rows = query.query_map(params![id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
            parts.push(row?);
        }
    }
    let mut query = conn.prepare(
        "SELECT id,data FROM message WHERE session_id=?1 ORDER BY sequence IS NULL, sequence, time_created, rowid",
    )?;
    let rows = query.query_map(params![id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut messages = Vec::new();
    for row in rows {
        let (message_id, data) = row?;
        let part_json: Vec<&str> = parts
            .iter()
            .filter(|(owner, _, _)| *owner == message_id)
            .map(|(_, _, data)| data.as_str())
            .collect();
        let raw = format!("{{\"info\":{data},\"parts\":[{}]}}", part_json.join(","));
        let mut message = message_from_ts_json(&raw)?;
        message["info"]["id"] = message_id.clone().into();
        message["info"]["sessionID"] = id.into();
        let owned: Vec<&String> = parts
            .iter()
            .filter(|(owner, _, _)| *owner == message_id)
            .map(|(_, part_id, _)| part_id)
            .collect();
        if let Some(list) = message["parts"].as_array_mut() {
            for (part, part_id) in list.iter_mut().zip(owned) {
                part["id"] = part_id.clone().into();
                part["messageID"] = message_id.clone().into();
                part["sessionID"] = id.into();
            }
        }
        messages.push(message);
    }
    Ok(Some((info, messages)))
}
