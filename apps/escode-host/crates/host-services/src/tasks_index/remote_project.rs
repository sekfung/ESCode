//! `remote-projects` 本机入口表（`hosts` + `remote_project_entries`）。
//!
//! 对齐上游 `services/src/remote-projects/index.ts`：本机 SQLite 是“用户添加了哪些远端项目”
//! 的唯一权威；只保存入口基础信息（target / membership / 缓存名称与 roots），不保存远端任务、
//! 消息或待同步操作。连接运行态与凭据仍归 Source registry / 连接管理器。

use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use uuid::Uuid;

use super::project_read::transaction;
use super::remote_project_rows::{
    entry_from_row, host_from_row, next_version, parse_json_column, required_text, select_all,
    select_optional,
};

const LOCAL_HOST: &str = "local";
/// 上游 `LEGACY_SINGLE_ROOT_PROJECT_PREFIX`：旧单根项目入口的缓存标识。
const LEGACY_SINGLE_ROOT_PROJECT_PREFIX: &str = "legacy-single-root:";
const MAX_REMOTE_HOSTS: i64 = 20;

/// 上游 `readRemoteProjectDirectory`：不含连接运行态，调用方负责事务。
pub(super) fn read_directory(connection: &Connection) -> Result<Value> {
    let hosts = select_all(
        connection,
        "SELECT * FROM hosts WHERE host_id<>'local' AND target_json IS NOT NULL ORDER BY position,host_id",
        &[],
    )?
    .iter()
    .map(host_from_row)
    .collect::<Result<Vec<_>>>()?;
    let entries = select_all(
        connection,
        "SELECT * FROM remote_project_entries ORDER BY rowid",
        &[],
    )?
    .iter()
    .map(entry_from_row)
    .collect::<Result<Vec<_>>>()?;
    Ok(json!({ "hosts": hosts, "entries": entries }))
}

fn assert_host_capacity(connection: &Connection) -> Result<()> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM hosts WHERE host_id<>'local' AND target_json IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    if count >= MAX_REMOTE_HOSTS {
        bail!("remote-project.host-limit");
    }
    Ok(())
}

fn require_remote(id: &str) -> Result<()> {
    if id.trim().is_empty() || id == LOCAL_HOST {
        bail!("remote-project.local-host");
    }
    Ok(())
}

fn target_object(input: &Value) -> Result<Value> {
    let target = input["target"].clone();
    if !target.is_object() || target["kind"].as_str().is_none_or(str::is_empty) {
        bail!("remote-project.invalid-target");
    }
    Ok(target)
}

pub(super) fn save_host(connection: &Connection, input: &Value) -> Result<Value> {
    let id = required_text(input, "id", "remote-project.invalid-host")?;
    require_remote(&id)?;
    let target = target_object(input)?;
    transaction(connection, true, || {
        let existing: Option<String> = connection
            .query_row(
                "SELECT membership_id FROM hosts WHERE host_id=? AND target_json IS NOT NULL",
                [&id],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_none() {
            assert_host_capacity(connection)?;
        }
        let membership = existing.unwrap_or_else(|| Uuid::new_v4().to_string());
        connection.execute(
            "INSERT INTO hosts(host_id,kind,target_json,membership_id,position) VALUES(?,?,?,?,(SELECT coalesce(max(position),-1)+1 FROM hosts))
             ON CONFLICT(host_id) DO UPDATE SET kind=excluded.kind,target_json=excluded.target_json,membership_id=excluded.membership_id",
            params![id, target["kind"].as_str().unwrap_or_default(), target.to_string(), membership],
        )?;
        let directory = read_directory(connection)?;
        directory["hosts"]
            .as_array()
            .and_then(|hosts| hosts.iter().find(|host| host["id"] == json!(id)).cloned())
            .ok_or_else(|| anyhow::anyhow!("remote-project.host-not-found"))
    })
}

/// 从 `remote:server:<id>:<path>` 提取稳定 Source id。
fn source_id_from_host_identity(identity: &str) -> Option<&str> {
    let rest = identity.strip_prefix("remote:server:")?;
    let (source_id, _) = rest.split_once(':')?;
    (!source_id.is_empty()).then_some(source_id)
}

pub(super) fn bind_host(connection: &Connection, input: &Value) -> Result<Value> {
    let id = required_text(input, "id", "remote-project.invalid-host")?;
    require_remote(&id)?;
    let membership = required_text(input, "membershipId", "remote-project.invalid-membership")?;
    let resolved_id = required_text(input, "resolvedId", "remote-project.invalid-source")?;
    if !resolved_id.starts_with("remote:server:") {
        bail!("remote-project.invalid-source");
    }
    let create_distinct = input["createDistinctHost"].as_bool().unwrap_or(false);
    let result = transaction(connection, true, || {
        let source = select_optional(
            connection,
            "SELECT * FROM hosts WHERE host_id=? AND membership_id=? AND target_json IS NOT NULL",
            &[&id, &membership],
        )?;
        let Some(source) = source else {
            return Ok(Value::Null);
        };
        let source_target = parse_json_column(&source, "target_json")?;
        // URL 在首次握手前也使用 server 前缀，只有固化 serverId 后才是稳定 Source。
        let url_alias = source_target["kind"] == json!("server")
            && source_target["serverId"].as_str().is_none_or(str::is_empty);
        let distinct_source = id.starts_with("remote:server:") && id != resolved_id && !url_alias;
        if distinct_source && !create_distinct {
            bail!("remote-project.source-identity-changed");
        }
        let target_json = if source_target["kind"] == json!("server") {
            let mut target = source_target.as_object().cloned().unwrap_or_default();
            target.insert(
                "serverId".to_owned(),
                json!(source_id_from_host_identity(&resolved_id).unwrap_or_default()),
            );
            Value::Object(target)
        } else {
            source_target.clone()
        };
        if distinct_source {
            let retained = read_directory(connection)?["hosts"]
                .as_array()
                .and_then(|hosts| {
                    hosts
                        .iter()
                        .find(|host| host["id"] == json!(resolved_id))
                        .cloned()
                });
            if let Some(retained) = retained {
                return Ok(retained);
            }
            assert_host_capacity(connection)?;
            // 重装后的 Source 属于新设备身份，确认只新增连接，不搬旧入口。
            connection.execute(
                "INSERT INTO hosts(host_id,kind,target_json,membership_id,position) VALUES(?,?,?,?,(SELECT coalesce(max(position),-1)+1 FROM hosts))
                 ON CONFLICT(host_id) DO UPDATE SET kind=excluded.kind,target_json=excluded.target_json,membership_id=excluded.membership_id",
                params![resolved_id, source["kind"].as_str().unwrap_or_default(), target_json.to_string(), Uuid::new_v4().to_string()],
            )?;
            return Ok(read_directory(connection)?["hosts"]
                .as_array()
                .and_then(|hosts| {
                    hosts
                        .iter()
                        .find(|host| host["id"] == json!(resolved_id))
                        .cloned()
                })
                .unwrap_or(Value::Null));
        }
        if id != resolved_id {
            if select_optional(
                connection,
                "SELECT 1 FROM projects WHERE host_id IN (?,?) LIMIT 1",
                &[&id, &resolved_id],
            )?
            .is_some()
            {
                bail!("remote-project.authoritative-host");
            }
            let retained = select_optional(
                connection,
                "SELECT 1 FROM hosts WHERE host_id=? AND target_json IS NOT NULL",
                &[&resolved_id],
            )?;
            if retained.is_none() {
                connection.execute(
                    "INSERT INTO hosts(host_id,kind,target_json,membership_id,position,last_connection_status,last_connection_error,last_opened_at)
                     SELECT ?,kind,target_json,membership_id,position,last_connection_status,last_connection_error,last_opened_at FROM hosts WHERE host_id=?
                     ON CONFLICT(host_id) DO UPDATE SET kind=excluded.kind,target_json=excluded.target_json,membership_id=excluded.membership_id",
                    params![resolved_id, id],
                )?;
            }
            // 同一物理 Source 的重复入口保留目标 entry ID，旧响应随旧 ID 一起失效。
            connection.execute(
                "DELETE FROM remote_project_entries WHERE host_id=? AND remote_project_id IS NOT NULL
                 AND remote_project_id IN (SELECT remote_project_id FROM remote_project_entries WHERE host_id=?)",
                params![id, resolved_id],
            )?;
            connection.execute(
                "UPDATE remote_project_entries SET host_id=? WHERE host_id=?",
                params![resolved_id, id],
            )?;
            connection.execute("DELETE FROM hosts WHERE host_id=?", params![id])?;
        }
        if source_target["kind"] == json!("server") {
            connection.execute(
                "UPDATE hosts SET target_json=? WHERE host_id=? AND membership_id=?",
                params![target_json.to_string(), resolved_id, membership],
            )?;
        }
        Ok(read_directory(connection)?["hosts"]
            .as_array()
            .and_then(|hosts| {
                hosts
                    .iter()
                    .find(|host| host["id"] == json!(resolved_id))
                    .cloned()
            })
            .unwrap_or(Value::Null))
    })?;
    Ok(result)
}

pub(super) fn remove_host(connection: &Connection, id: &str) -> Result<bool> {
    require_remote(id)?;
    transaction(connection, true, || {
        // 本机移除连接不能把权威项目级联删除；迁移后的本机表不应含远端权威项目。
        if select_optional(
            connection,
            "SELECT 1 FROM projects WHERE host_id=? LIMIT 1",
            &[&id],
        )?
        .is_some()
        {
            bail!("remote-project.authoritative-host");
        }
        connection.execute(
            "DELETE FROM remote_project_entries WHERE host_id=?",
            params![id],
        )?;
        let changes = connection.execute("DELETE FROM hosts WHERE host_id=?", params![id])?;
        Ok(changes > 0)
    })
}

fn cache_project(input: &Value) -> Result<(String, String, Value)> {
    let project = &input["project"];
    let project_id = required_text(project, "projectId", "remote-project.invalid-project")?;
    let name = required_text(project, "name", "remote-project.invalid-project")?;
    let roots = project["roots"].clone();
    if !roots.is_array() {
        bail!("remote-project.invalid-project");
    }
    Ok((project_id, name, roots))
}

pub(super) fn add_entry(connection: &Connection, input: &Value) -> Result<Value> {
    let host_id = required_text(input, "hostId", "remote-project.invalid-host")?;
    require_remote(&host_id)?;
    let (project_id, name, roots) = cache_project(input)?;
    let expected_membership = input["expectedMembershipId"].as_str().map(str::to_owned);
    transaction(connection, true, || {
        if select_optional(
            connection,
            "SELECT 1 FROM hosts WHERE host_id=? AND target_json IS NOT NULL",
            &[&host_id],
        )?
        .is_none()
        {
            bail!("remote-project.host-not-found");
        }
        if let Some(expected) = expected_membership.as_deref() {
            let membership: Option<String> = connection
                .query_row(
                    "SELECT membership_id FROM hosts WHERE host_id=?",
                    [&host_id],
                    |row| row.get(0),
                )
                .optional()?;
            // 连接期间删除并重新添加同一设备后，旧向导不得给新 membership 登记入口。
            if membership.as_deref() != Some(expected) {
                bail!("remote-project.host-removed");
            }
        }
        let old = select_optional(
            connection,
            "SELECT * FROM remote_project_entries WHERE host_id=? AND remote_project_id=?",
            &[&host_id, &project_id],
        )?;
        let entry_id = old
            .as_ref()
            .and_then(|old| old["entry_id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let previous_version = old.as_ref().and_then(|old| old["last_synced_at"].as_i64());
        connection.execute(
            "INSERT INTO remote_project_entries(entry_id,host_id,remote_project_id,cached_name,cached_roots_json,last_synced_at) VALUES(?,?,?,?,?,?)
             ON CONFLICT(entry_id) DO UPDATE SET cached_name=excluded.cached_name,cached_roots_json=excluded.cached_roots_json,last_synced_at=excluded.last_synced_at",
            params![entry_id, host_id, project_id, name, roots.to_string(), next_version(previous_version)],
        )?;
        let row = select_optional(
            connection,
            "SELECT * FROM remote_project_entries WHERE entry_id=?",
            &[&entry_id],
        )?
        .ok_or_else(|| anyhow::anyhow!("remote-project.entry-not-found"))?;
        entry_from_row(&row)
    })
}

pub(super) fn refresh_entry(connection: &Connection, input: &Value) -> Result<bool> {
    let entry_id = required_text(input, "entryId", "remote-project.invalid-entry")?;
    let expected = input["expectedLastSyncedAt"].as_i64();
    let (project_id, name, roots) = cache_project(input)?;
    transaction(connection, true, || {
        let legacy_like = format!("{LEGACY_SINGLE_ROOT_PROJECT_PREFIX}%");
        let previous = select_optional(
            connection,
            "SELECT * FROM remote_project_entries
             WHERE entry_id=? AND last_synced_at IS ? AND (remote_project_id IS NULL OR remote_project_id=? OR remote_project_id LIKE ?)",
            &[&entry_id, &expected, &project_id, &legacy_like],
        )?;
        let Some(previous) = previous else {
            return Ok(false);
        };
        let previous_project_id = previous["remote_project_id"].as_str().map(str::to_owned);
        let legacy_entry = previous_project_id
            .as_deref()
            .is_none_or(|id| id.starts_with(LEGACY_SINGLE_ROOT_PROJECT_PREFIX));
        if legacy_entry
            && select_optional(
                connection,
                "SELECT 1 FROM remote_project_entries WHERE host_id=? AND remote_project_id=? AND entry_id<>?",
                &[&previous["host_id"].as_str().unwrap_or_default(), &project_id, &entry_id],
            )?
            .is_some()
        {
            // 多个旧 cwd 可解析到同一项目；保留已解析 entry ID，删除当前通过 CAS 的旧入口。
            connection.execute("DELETE FROM remote_project_entries WHERE entry_id=?", params![entry_id])?;
            return Ok(true);
        }
        let roots_json = roots.to_string();
        connection.execute(
            "UPDATE remote_project_entries SET remote_project_id=?,cached_name=?,cached_roots_json=?,last_synced_at=? WHERE entry_id=?",
            params![project_id, name, roots_json, next_version(expected), entry_id],
        )?;
        Ok(true)
    })
}

pub(super) fn remove_entry(connection: &Connection, id: &str) -> Result<bool> {
    if id.trim().is_empty() {
        bail!("remote-project.invalid-entry");
    }
    let changes = connection.execute(
        "DELETE FROM remote_project_entries WHERE entry_id=?",
        params![id],
    )?;
    Ok(changes > 0)
}
