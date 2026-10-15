//! `escode-task` 的分组视图写面（TS `escodeTaskServiceAdapter.ts:2765-2820` + `taskIndexRepo.ts` 分组写）：
//! `createTaskGroup` / `renameTaskGroup` / `updateTaskGroupColor` / `deleteTaskGroup` / `applyGroupedTaskViewOrder`。
//!
//! 事件：rename / recolor / delete / apply 成功后，对入参 `workspaceScopes` 的每个 scope 广播
//! `workspace_task_list_changed(reason = "task_meta_changed", 无 taskMeta)`——分组结构变更没有单任务 meta，
//! Renderer 靠它重拉分组视图；`createTaskGroup` 不广播（TS 同）。
//!
//! 登记差异（docs/specs/rust-u2-tasks-index.md §6.8）：`applyGroupedTaskViewOrder` 的回包 `nodes` 恒为空——
//! TS 回包来自 `tasks` 表 join 的旧视图，Renderer 已明确不采信（`useGroupedTaskView.ts`：持久化成功后以「结构 +
//! sessions-index」重建），TS 自己也把它标注为待收敛的过渡面。

use escode_host_services::tasks_index::write_groups::{
    GroupInput, OrderInput, TaskRefInput, TopNode,
};
use serde_json::{Value, json};

use crate::domains_task_state::{StateContext, index_error, method, with_index};
use crate::domains_task_support::GLM_PROVIDER;
use crate::session::{MethodHandler, RpcError};
use crate::wire::Wire;
use crate::workspace_events::Target;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn text(params: &Value, key: &str) -> String {
    params[key].as_str().unwrap_or_default().to_owned()
}

fn scopes(params: &Value) -> Vec<(String, Option<String>)> {
    params["workspaceScopes"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|scope| {
            (
                text(scope, "workspacePath"),
                scope["workspaceIdentity"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// grouped 结构没有单任务 meta：沿用 `task_meta_changed` 驱动 Renderer 重拉。
fn announce(context: &StateContext, scopes: &[(String, Option<String>)]) {
    for (path, identity) in scopes {
        context.events.task_list_changed(
            &Target {
                workspace_path: path,
                workspace_identity: identity.as_deref(),
                task_id: None,
            },
            None,
            "task_meta_changed",
            None,
        );
    }
}

fn parse_top_node(node: &Value) -> Result<TopNode, RpcError> {
    match node["type"].as_str() {
        Some("group") => Ok(TopNode::Group(text(node, "groupId"))),
        Some("task") => Ok(TopNode::Task(TaskRefInput::from_value(&node["task"]))),
        _ => Err(RpcError::new(
            "InvalidRequest",
            "Grouped task order 顶层节点类型无效",
        )),
    }
}

pub(super) fn group_methods(context: &StateContext) -> Vec<(&'static str, MethodHandler)> {
    vec![
        (
            "createTaskGroup",
            method(context, |context, params| async move {
                let id = format!("task-group-{}", uuid::Uuid::new_v4());
                let title = params["title"].as_str().map(str::to_owned);
                let color = params["color"].as_str().map(str::to_owned);
                let group = with_index(&context, move |index| {
                    index.create_task_group(&id, title.as_deref(), color.as_deref(), now_ms())
                })
                .await?
                .map_err(|error| index_error(&error))?;
                Ok(Wire::Object(group))
            }),
        ),
        (
            "renameTaskGroup",
            method(context, |context, params| async move {
                let (id, title) = (text(&params, "groupId"), text(&params, "title"));
                let group = with_index(&context, move |index| {
                    index.rename_task_group(&id, &title, now_ms())
                })
                .await?
                .map_err(|error| index_error(&error))?;
                announce(&context, &scopes(&params));
                Ok(Wire::Object(group))
            }),
        ),
        (
            "updateTaskGroupColor",
            method(context, |context, params| async move {
                let (id, color) = (text(&params, "groupId"), text(&params, "color"));
                let group = with_index(&context, move |index| {
                    index.update_task_group_color(&id, &color, now_ms())
                })
                .await?
                .map_err(|error| index_error(&error))?;
                announce(&context, &scopes(&params));
                Ok(Wire::Object(group))
            }),
        ),
        (
            "deleteTaskGroup",
            method(context, |context, params| async move {
                let id = text(&params, "groupId");
                with_index(&context, move |index| index.delete_task_group(&id))
                    .await?
                    .map_err(|error| index_error(&error))?;
                announce(&context, &scopes(&params));
                Ok(Wire::Undefined)
            }),
        ),
        (
            "applyGroupedTaskViewOrder",
            method(context, |context, params| async move {
                let top_level = params["topLevelNodes"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(parse_top_node)
                    .collect::<Result<Vec<_>, _>>()?;
                let groups: Vec<GroupInput> = params["groups"]
                    .as_array()
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .map(|group| GroupInput {
                        group_id: text(group, "groupId"),
                        task_refs: group["taskRefs"]
                            .as_array()
                            .map_or(&[][..], Vec::as_slice)
                            .iter()
                            .map(TaskRefInput::from_value)
                            .collect(),
                    })
                    .collect();
                let scope_list = scopes(&params);
                let owned = scope_list.clone();
                with_index(&context, move |index| {
                    let borrowed: Vec<(&str, Option<&str>)> = owned
                        .iter()
                        .map(|(path, identity)| (path.as_str(), identity.as_deref()))
                        .collect();
                    index.apply_grouped_task_view_order(
                        &OrderInput {
                            workspace_scopes: &borrowed,
                            top_level_nodes: top_level,
                            groups,
                            // 回包与校验都继承列表查询的 glm provider 边界（旧第三方 provider 的排序残留不得回到 UI）。
                            provider: Some(GLM_PROVIDER),
                        },
                        now_ms(),
                    )
                })
                .await?
                .map_err(|error| index_error(&error))?;
                announce(&context, &scope_list);
                Ok(Wire::Object(json!({"nodes": []})))
            }),
        ),
    ]
}
