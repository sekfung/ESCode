//! 写路径（从 storage.rs 拆出）：commit / write 与 SessionWrite 投影。

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::domain::session::Session;

pub(super) fn commit(
    conn: &mut Connection,
    workspace: &str,
    session: Option<SessionWrite>,
    ack: Option<(String, Value)>,
) -> Result<()> {
    let tx = conn.transaction()?;
    write(&tx, workspace, session, ack)?;
    tx.commit()?;
    Ok(())
}
pub(super) fn write(
    tx: &Connection,
    workspace: &str,
    session: Option<SessionWrite>,
    ack: Option<(String, Value)>,
) -> Result<()> {
    if let Some(session) = session {
        // 截断前保存原 command 的已启动事实，旧 ACK 不能因为显示行消失被误判为丢弃。
        if session.rewrite {
            tx.execute("INSERT OR IGNORE INTO rust_started SELECT workspace,session,json_extract(body,'$.sourceCommandId') FROM rust_row WHERE workspace=?1 AND session=?2 AND json_extract(body,'$.sourceCommandId') IS NOT NULL",params![workspace,session.id])?;
            for table in ["rust_row", "rust_message"] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE workspace=?1 AND session=?2"),
                    params![workspace, session.id],
                )?;
            }
        }
        for command in &session.started {
            tx.execute(
                "INSERT OR IGNORE INTO rust_started VALUES(?1,?2,?3)",
                params![workspace, session.id, command],
            )?;
        }
        tx.execute("INSERT INTO rust_session VALUES(?1,?2,?3) ON CONFLICT(workspace,id) DO UPDATE SET body=excluded.body",params![workspace,session.id,session.metadata])?;
        session.history.save(tx, workspace, &session.id)?;
        for (table, start, values) in [
            ("rust_row", session.row_start, session.rows),
            ("rust_message", session.message_start, session.messages),
        ] {
            let mut statement = tx.prepare_cached(&format!("INSERT INTO {table} VALUES(?1,?2,?3,?4) ON CONFLICT(workspace,session,ordinal) DO UPDATE SET body=excluded.body WHERE body IS NOT excluded.body"))?;
            for (offset, value) in values.into_iter().enumerate() {
                statement.execute(params![
                    workspace,
                    session.id,
                    (start + offset) as i64,
                    value
                ])?;
            }
        }
        for (key, ack) in session.pending_acks {
            tx.execute("INSERT INTO rust_command VALUES(?1,?2,?3) ON CONFLICT(workspace,key) DO UPDATE SET ack=excluded.ack", params![workspace,key,serde_json::to_string(&ack)?])?;
        }
        if let Some((key, ack)) = session.creation_ack {
            tx.execute(
                "INSERT INTO rust_command VALUES(?1,?2,?3) ON CONFLICT(workspace,key) DO NOTHING",
                params![workspace, key, serde_json::to_string(&ack)?],
            )?;
        }
    }
    if let Some((key, ack)) = ack {
        tx.execute("INSERT INTO rust_command VALUES(?1,?2,?3) ON CONFLICT(workspace,key) DO UPDATE SET ack=excluded.ack",params![workspace,key,serde_json::to_string(&ack)?])?;
    }
    Ok(())
}

pub(super) struct SessionWrite {
    history: super::storage_history::Write,
    rewrite: bool,
    id: String,
    metadata: String,
    row_start: usize,
    rows: Vec<String>,
    started: Vec<String>,
    message_start: usize,
    messages: Vec<String>,
    creation_ack: Option<(String, Value)>,
    pending_acks: BTreeMap<String, Value>,
}
impl SessionWrite {
    pub(super) fn new(session: &Session) -> Result<Self> {
        let row_start = if session.history_rewrite {
            0
        } else {
            session.saved_rows.min(session.current_rows_start())
        };
        let message_start = if session.history_rewrite {
            0
        } else {
            session.saved_messages
        };
        Ok(Self {
            history: super::storage_history::Write::new(session)?,
            rewrite: session.history_rewrite,
            id: session.id.clone(),
            metadata: serde_json::to_string(session)?,
            row_start,
            rows: session.rows[row_start..]
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<_, _>>()?,
            started: session.rows[row_start..]
                .iter()
                .filter_map(|r| r["sourceCommandId"].as_str().map(str::to_owned))
                .collect(),
            message_start,
            messages: session.messages[message_start..]
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<_, _>>()?,
            creation_ack: session.creation_ack.clone(),
            pending_acks: session.pending_acks.clone(),
        })
    }
}
