//! 项目权限规则：TS `permission-rules-persistence.ts` 的 get/setProjectPermission 对应实现。
//! 规则集按 workspace 作用域持久化，「总是允许」在项目内跨会话生效。
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

pub(super) fn load(conn: &Connection, workspace: &str) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT rules FROM rust_project_rule WHERE workspace=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(None) };
    Ok(Some(
        serde_json::from_str(&raw).context("Invalid persisted project rules")?,
    ))
}

pub(super) fn save(conn: &mut Connection, workspace: &str, rules: &Value) -> Result<()> {
    conn.execute(
        "INSERT INTO rust_project_rule(workspace,rules) VALUES(?1,?2)
         ON CONFLICT(workspace) DO UPDATE SET rules=excluded.rules",
        rusqlite::params![workspace, rules.to_string()],
    )?;
    Ok(())
}
