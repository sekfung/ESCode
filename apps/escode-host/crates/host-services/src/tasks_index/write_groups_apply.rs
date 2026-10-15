//! `applyGroupedTaskViewOrder` 的写入（从 `write_groups.rs` 拆出以守住单文件行数上限；规则见该文件头注释）。

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use super::write_groups::{BOOTSTRAP_ONCE_KEY, ORDER_STEP, rejected, sqlite, task_order_node_key};
use super::write_state::transaction;
use super::{TasksIndexError, workspace_key};

/// 一个 task 引用（`{workspacePath, workspaceIdentity?, taskId}`）。
#[derive(Clone, Debug)]
pub struct TaskRefInput {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub task_id: String,
}

impl TaskRefInput {
    pub fn from_value(value: &Value) -> Self {
        Self {
            workspace_path: value["workspacePath"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            workspace_identity: value["workspaceIdentity"].as_str().map(str::to_owned),
            task_id: value["taskId"].as_str().unwrap_or_default().to_owned(),
        }
    }

    fn key(&self) -> String {
        workspace_key(&self.workspace_path, self.workspace_identity.as_deref())
    }
}

/// 顶层节点：group 或顶层 task。
#[derive(Clone, Debug)]
pub enum TopNode {
    Group(String),
    Task(TaskRefInput),
}

pub struct GroupInput {
    pub group_id: String,
    pub task_refs: Vec<TaskRefInput>,
}

/// `applyGroupedTaskViewOrder` 的入参。
pub struct OrderInput<'a> {
    pub workspace_scopes: &'a [(&'a str, Option<&'a str>)],
    pub top_level_nodes: Vec<TopNode>,
    pub groups: Vec<GroupInput>,
    /// 带 provider 时只接受该 provider 的 task（旧 provider 引用静默跳过）。
    pub provider: Option<&'a str>,
}

/// 校验一个 task 引用：`Ok(Some(key))` 可用；`Ok(None)` 旧 provider 遗留（跳过）；`Err` 非法。
fn validate_task(
    connection: &Connection,
    scope_keys: &BTreeSet<String>,
    provider: Option<&str>,
    task: &TaskRefInput,
) -> Result<Option<String>, TasksIndexError> {
    let key = task.key();
    if !scope_keys.contains(&key) {
        return Err(rejected("Grouped task order 包含当前 scope 外的 task"));
    }
    let row: Option<(i64, i64, i64, Option<String>)> = connection
        .query_row(
            "SELECT deleted, archived, pinned, provider FROM tasks WHERE workspace_key = ?1 AND task_id = ?2",
            params![key, task.task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(sqlite)?;
    match row {
        Some((0, 0, 0, row_provider)) => {
            if provider.is_some_and(|expected| row_provider.as_deref() != Some(expected)) {
                return Ok(None);
            }
            Ok(Some(key))
        }
        _ => Err(rejected("Grouped task order 包含不可见 task")),
    }
}

/// TS `applyGroupedTaskViewOrder` 的写入部分（回包由调用方处理）。
pub fn apply_grouped_task_view_order(
    connection: &Connection,
    input: &OrderInput<'_>,
    now: i64,
) -> Result<(), TasksIndexError> {
    let scope_keys: BTreeSet<String> = input
        .workspace_scopes
        .iter()
        .map(|(path, identity)| workspace_key(path, *identity))
        .filter(|key| !key.trim().is_empty())
        .collect();
    let group_ids: BTreeSet<String> = {
        let mut statement = connection
            .prepare("SELECT group_id FROM task_groups")
            .map_err(sqlite)?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sqlite)?
            .collect::<Result<_, _>>()
            .map_err(sqlite)?
    };

    let mut top_level_task_keys: Vec<(String, String)> = Vec::new();
    let mut visible_top: Vec<&TopNode> = Vec::new();
    for node in &input.top_level_nodes {
        match node {
            TopNode::Group(id) => {
                if !group_ids.contains(id) {
                    return Err(rejected("Grouped task order 包含不存在的 group"));
                }
                visible_top.push(node);
            }
            TopNode::Task(task) => {
                let Some(key) = validate_task(connection, &scope_keys, input.provider, task)?
                else {
                    continue;
                };
                top_level_task_keys.push((key, task.task_id.clone()));
                visible_top.push(node);
            }
        }
    }
    let mut grouped_keys: BTreeSet<(String, String)> = BTreeSet::new();
    let mut visible_groups: Vec<(&str, Vec<&TaskRefInput>)> = Vec::new();
    for group in &input.groups {
        if !group_ids.contains(&group.group_id) {
            return Err(rejected("Grouped task order 包含不存在的 group"));
        }
        let mut refs = Vec::new();
        for task in &group.task_refs {
            let Some(key) = validate_task(connection, &scope_keys, input.provider, task)? else {
                continue;
            };
            if !grouped_keys.insert((key, task.task_id.clone())) {
                return Err(rejected(
                    "Grouped task order 不能让同一个 task 进入多个 group",
                ));
            }
            refs.push(task);
        }
        visible_groups.push((group.group_id.as_str(), refs));
    }

    let scoped_task_keys: Vec<(String, String)> = if scope_keys.is_empty() {
        Vec::new()
    } else {
        let placeholders = vec!["?"; scope_keys.len()].join(", ");
        let mut statement = connection
            .prepare(&format!(
                "SELECT workspace_key, task_id FROM tasks WHERE workspace_key IN ({placeholders})"
            ))
            .map_err(sqlite)?;
        statement
            .query_map(rusqlite::params_from_iter(scope_keys.iter()), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(sqlite)?
            .collect::<Result<_, _>>()
            .map_err(sqlite)?
    };

    transaction(connection, || {
        // 用户显式保存排序后不再做 workspace 自动分组初始化：写全局 marker。
        connection
            .execute(
                "INSERT INTO task_group_workspace_bootstraps (workspace_key, group_id, created_at, updated_at)
                 VALUES (?1, NULL, ?2, ?2)
                 ON CONFLICT(workspace_key) DO UPDATE SET updated_at = excluded.updated_at",
                params![BOOTSTRAP_ONCE_KEY, now],
            )
            .map_err(sqlite)?;
        // 顶层 task 退出 group。
        for (key, task_id) in &top_level_task_keys {
            connection
                .execute(
                    "DELETE FROM task_group_members WHERE workspace_key = ?1 AND task_id = ?2",
                    params![key, task_id],
                )
                .map_err(sqlite)?;
        }
        for (group_id, refs) in &visible_groups {
            for (index, task) in refs.iter().enumerate() {
                let order = (i64::try_from(index).unwrap_or(0) + 1) * ORDER_STEP;
                connection
                    .execute(
                        "INSERT INTO task_group_members (
                           group_id, workspace_key, workspace_path, workspace_identity, task_id,
                           sort_order, added_at, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7)
                         ON CONFLICT(workspace_key, task_id) DO UPDATE SET
                           group_id = excluded.group_id,
                           workspace_path = excluded.workspace_path,
                           workspace_identity = excluded.workspace_identity,
                           sort_order = excluded.sort_order,
                           updated_at = excluded.updated_at",
                        params![
                            group_id,
                            task.key(),
                            task.workspace_path,
                            task.workspace_identity,
                            task.task_id,
                            order,
                            now
                        ],
                    )
                    .map_err(sqlite)?;
            }
        }
        // 一次提交最终顶层排序：清全部 group 排序 + 当前 scope 内 task 排序（其它 scope 的混排位置不得误删）。
        connection
            .execute(
                "DELETE FROM task_group_view_node_orders WHERE node_type = 'group'",
                [],
            )
            .map_err(sqlite)?;
        for (key, task_id) in &scoped_task_keys {
            connection
                .execute(
                    "DELETE FROM task_group_view_node_orders WHERE node_type = 'task' AND (node_key = ?1 OR node_key = ?2)",
                    params![task_order_node_key(key, task_id), key],
                )
                .map_err(sqlite)?;
        }
        for (index, node) in visible_top.iter().enumerate() {
            let (node_type, node_key) = match node {
                TopNode::Group(id) => ("group", id.clone()),
                TopNode::Task(task) => ("task", task_order_node_key(&task.key(), &task.task_id)),
            };
            let order = (i64::try_from(index).unwrap_or(0) + 1) * ORDER_STEP;
            connection
                .execute(
                    "INSERT INTO task_group_view_node_orders (node_type, node_key, sort_order, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![node_type, node_key, order, now],
                )
                .map_err(sqlite)?;
        }
        Ok(())
    })
}
