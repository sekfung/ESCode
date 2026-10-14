use crate::domain::session::Session;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

type StoredWorkspace = (Vec<Session>, BTreeMap<String, Value>);
pub(super) enum Operation {
    Index(String, oneshot::Sender<Result<BTreeMap<String, Value>>>),
    Ack(String, String, oneshot::Sender<Result<Option<Value>>>),
    List(
        crate::domain::session_listing::ListParams,
        (String, String),
        oneshot::Sender<Result<Vec<crate::domain::session_listing::SessionListing>>>,
    ),
    Import(
        super::legacy_storage::ImportRequest,
        oneshot::Sender<Result<()>>,
    ),
    Load(String, oneshot::Sender<Result<StoredWorkspace>>),
    LoadSession(String, String, oneshot::Sender<Result<Option<Session>>>),
    DiscardDraft(String, String, (String, Value), oneshot::Sender<Result<()>>),
    Commit(
        String,
        Option<Box<super::storage_write::SessionWrite>>,
        Option<(String, Value)>,
        oneshot::Sender<Result<()>>,
    ),
    /// 项目权限规则（按 workspace 作用域），「总是允许」写入后由后续判定读取。
    ProjectRules(String, oneshot::Sender<Result<Option<Value>>>),
    SaveProjectRules(String, Value, oneshot::Sender<Result<()>>),
    SessionContext(
        String,
        oneshot::Sender<Result<Option<SessionContextSource>>>,
    ),
    /// 动态工作流 run journal 的读面（docs/specs/rust-dynamic-workflow.md 第 4 期前置）：
    /// `workflows/runs` 与将来的引擎共用同一份 dwf_* 表。
    WorkflowRuns(
        crate::domain::dwf_journal::RunQuery,
        oneshot::Sender<Result<Vec<crate::domain::dwf_journal::JournalRun>>>,
    ),
    /// 模型用量（storage_usage.rs）：记录事实或按会话聚合。
    Usage(Value, oneshot::Sender<Result<Value>>),
}
pub(super) type SessionContextSource = escode_cli_domain::session_context::SessionSource;

/// 工作区 owner 锁被另一个进程持有：所有权冲突，等持有者退出后可重试。
#[derive(Debug)]
pub struct WorkspaceOwned;
impl std::fmt::Display for WorkspaceOwned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Workspace runtime is already owned or cannot be locked")
    }
}
impl std::error::Error for WorkspaceOwned {}
/// app-server 因 [`WorkspaceOwned`] 退出时的进程退出码（sysexits EX_TEMPFAIL）。Host 协议契约的一部分。
pub const WORKSPACE_OWNED_EXIT_CODE: i32 = 75;

#[derive(Clone)]
pub struct Store {
    pub(super) tx: mpsc::Sender<Operation>,
    pub(super) attachment_root: PathBuf,
}

impl Store {
    pub async fn lock_workspace(dir: PathBuf, workspace: String) -> Result<std::fs::File> {
        use sha2::{Digest, Sha256};
        tokio::task::spawn_blocking(move || {
            let key = format!("{:x}", Sha256::digest(workspace.as_bytes()));
            let target = dir.join(format!("workspace-{key}.lock"));
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&target)
                .with_context(|| {
                    format!("Workspace lock file unavailable: {}", target.display())
                })?;
            // SQLite 的写锁不能阻止第二个 actor 先读取旧状态并执行恢复，必须先锁 owner。
            // 被其他进程持有（WouldBlock）是可重试的所有权冲突，用 WorkspaceOwned 标记，入口据此给出
            // 专用退出码，Host 不能把它当成 Rust runtime 本身故障（docs/specs/rust-release-rollback.md）。
            match file.try_lock() {
                Ok(()) => Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => Err(WorkspaceOwned.into()),
                Err(std::fs::TryLockError::Error(error)) => Err(anyhow::Error::new(error)
                    .context("Workspace runtime is already owned or cannot be locked")),
            }
        })
        .await?
    }
    pub async fn open(path: PathBuf) -> Result<Self> {
        let attachment_root = path
            .parent()
            .context("Storage directory missing")?
            .join("attachments");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let (tx, mut rx) = mpsc::channel(64);
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let connection = (|| -> Result<Connection> {
                let conn = Connection::open(path)?;
                conn.busy_timeout(std::time::Duration::from_secs(5))?;
                conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
                    CREATE TABLE IF NOT EXISTS rust_session(workspace TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(workspace,id));
                    CREATE TABLE IF NOT EXISTS rust_history(workspace TEXT NOT NULL,session TEXT NOT NULL,kind TEXT NOT NULL,ordinal INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(workspace,session,kind,ordinal));
                    CREATE TABLE IF NOT EXISTS rust_started(workspace TEXT NOT NULL,session TEXT NOT NULL,command TEXT NOT NULL,PRIMARY KEY(workspace,session,command));
                    CREATE TABLE IF NOT EXISTS rust_command(workspace TEXT NOT NULL,key TEXT NOT NULL,ack TEXT NOT NULL,PRIMARY KEY(workspace,key));
                    CREATE TABLE IF NOT EXISTS rust_row(workspace TEXT NOT NULL,session TEXT NOT NULL,ordinal INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(workspace,session,ordinal));
                    CREATE TABLE IF NOT EXISTS rust_message(workspace TEXT NOT NULL,session TEXT NOT NULL,ordinal INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(workspace,session,ordinal));
                    CREATE TABLE IF NOT EXISTS rust_project_rule(workspace TEXT NOT NULL,rules TEXT NOT NULL,PRIMARY KEY(workspace));")?;
                super::dwf_journal::ensure_schema(&conn)?;
                super::storage_usage::ensure_schema(&conn)?;
                super::storage_listing::prepare(&conn)?;
                conn.execute_batch("CREATE INDEX IF NOT EXISTS rust_row_command ON rust_row(workspace,session,CASE WHEN json_valid(body) THEN json_extract(body,'$.sourceCommandId') END);")?;
                Ok(conn)
            })();
            let mut conn = match connection {
                Ok(c) => {
                    let _ = ready_tx.send(Ok(()));
                    c
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            // 最近一次导入的 TS 库：ReadSessionContext 对未导入会话只读回落。
            let mut legacy_source: Option<PathBuf> = None;
            while let Some(op) = rx.blocking_recv() {
                match op {
                    Operation::Index(workspace, reply) => {
                        let _ = reply.send(super::storage_index::index(&conn, &workspace));
                    }
                    Operation::Ack(workspace, key, reply) => {
                        let _ = reply.send(super::storage_index::ack(&mut conn, &workspace, &key));
                    }
                    Operation::List(params, owner, reply) => {
                        let _ = reply.send(super::storage_listing::list(
                            &conn,
                            &params,
                            (&owner.0, &owner.1),
                        ));
                    }
                    Operation::Import(request, reply) => {
                        legacy_source = Some(request.source.clone());
                        let _ = reply.send(super::legacy_storage::import(&mut conn, request));
                    }
                    Operation::Load(workspace, reply) => {
                        let _ = reply.send(super::storage_read::load(&conn, &workspace));
                    }
                    Operation::SessionContext(id, reply) => {
                        let source = legacy_source.as_deref();
                        let _ =
                            reply.send(super::storage_session_context::read(&conn, source, &id));
                    }
                    Operation::LoadSession(workspace, id, reply) => {
                        let _ =
                            reply.send(super::storage_read::load_session(&conn, &workspace, &id));
                    }
                    Operation::DiscardDraft(workspace, id, ack, reply) => {
                        let _ = reply.send(discard_draft(&mut conn, &workspace, &id, ack));
                    }
                    Operation::Commit(workspace, session, ack, reply) => {
                        let _ = reply.send(super::storage_write::commit(
                            &mut conn,
                            &workspace,
                            session.map(|s| *s),
                            ack,
                        ));
                    }
                    Operation::ProjectRules(workspace, reply) => {
                        let _ = reply.send(super::storage_project_rules::load(&conn, &workspace));
                    }
                    Operation::SaveProjectRules(workspace, rules, reply) => {
                        let _ = reply.send(super::storage_project_rules::save(
                            &mut conn, &workspace, &rules,
                        ));
                    }
                    Operation::WorkflowRuns(query, reply) => {
                        let _ = reply.send(super::dwf_journal::journal_runs(&conn, &query));
                    }
                    Operation::Usage(request, reply) => {
                        let _ = reply.send(super::storage_usage::handle(&conn, &request));
                    }
                }
            }
        });
        ready_rx.await.context("Storage worker stopped")??;
        Ok(Self {
            tx,
            attachment_root,
        })
    }
    pub async fn load(&self, workspace: &str) -> Result<(Vec<Session>, BTreeMap<String, Value>)> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Operation::Load(workspace.into(), tx)).await?;
        rx.await?
    }
    pub async fn import_ts(
        &self,
        source: PathBuf,
        workspace: String,
        cwd: String,
        dir: PathBuf,
        artifacts: PathBuf,
        cancel: CancellationToken,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Operation::Import(
                super::legacy_storage::ImportRequest {
                    source,
                    workspace,
                    cwd,
                    dir,
                    artifacts,
                    cancel,
                },
                tx,
            ))
            .await?;
        rx.await?
    }
    pub async fn commit(
        &self,
        workspace: &str,
        session: Option<&mut Session>,
        ack: Option<(String, Value)>,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Operation::Commit(
                workspace.into(),
                session
                    .as_deref()
                    .map(super::storage_write::SessionWrite::new)
                    .transpose()?
                    .map(Box::new),
                ack,
                tx,
            ))
            .await?;
        rx.await??;
        if let Some(session) = session {
            session.saved_inputs = session.history.inputs.len();
            session.saved_responses = session.history.responses.len();
            session.history_rewrite = false;
            session.saved_rows = session.rows.len();
            session.saved_messages = session.messages.len();
            session.pending_acks.clear();
        }
        Ok(())
    }
}
pub(super) fn load_items(
    conn: &Connection,
    table: &str,
    workspace: &str,
    id: &str,
) -> Result<Vec<Value>> {
    let mut query = conn.prepare_cached(&format!(
        "SELECT body FROM {table} WHERE workspace=?1 AND session=?2 ORDER BY ordinal"
    ))?;
    query
        .query_map(params![workspace, id], |row| row.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str(&row?)?))
        .collect()
}

fn discard_draft(
    conn: &mut Connection,
    workspace: &str,
    id: &str,
    ack: (String, Value),
) -> Result<()> {
    let tx = conn.transaction()?;
    // 关闭草稿不是真删历史；存储边界再次核查，避免 future caller 用过期 draft 状态误删首发。
    let Some(session) = super::storage_read::load_session(&tx, workspace, id)? else {
        // 预热草稿只活在内存；关闭不应为每次界面切换增加永久 ACK 或 WAL 写入。
        return Ok(());
    };
    anyhow::ensure!(
        session.rows.is_empty() && session.messages.is_empty(),
        "Cannot discard a persisted conversation as draft"
    );
    tx.execute(
        "DELETE FROM rust_session WHERE workspace=?1 AND id=?2",
        params![workspace, id],
    )?;
    tx.execute("INSERT INTO rust_command VALUES(?1,?2,?3) ON CONFLICT(workspace,key) DO UPDATE SET ack=excluded.ack", params![workspace,ack.0,serde_json::to_string(&ack.1)?])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
