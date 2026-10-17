//! `escode-task` 其余方法注册（S9–S12）：initialize / goalSession / Claude 导入 / Bot group。
//! 从 `domains_task.rs` 拆出以守住单文件行数上限。

use crate::domains_task_state::StateContext;
use crate::domains_task_support::RuntimeAccess;
use crate::session::MethodHandler;

pub(super) fn extra_methods(
    context: &StateContext,
    runtime: Option<RuntimeAccess>,
) -> Vec<(&'static str, MethodHandler)> {
    let mut methods = vec![
        (
            "initialize",
            super::domains_task_runtime::initialize_method(runtime.clone()),
        ),
        (
            "goalSession",
            super::domains_task_goal::goal_method(context),
        ),
    ];
    methods.extend(super::domains_task_import::import_methods(context));
    methods.extend(super::domains_task_bot::bot_methods(context));
    methods.extend(super::domains_task_runtime::usage_and_log_methods(
        runtime.clone(),
    ));
    // 关闭 workspace UI 的 `releaseWorkspacePreparation` 与 restart 同走 `disposeWorkspace`。
    methods.push((
        "restartWorkspaceProcess",
        super::domains_task_runtime::restart_workspace_process(runtime.clone()),
    ));
    methods.push((
        "releaseWorkspacePreparation",
        super::domains_task_runtime::restart_workspace_process(runtime),
    ));
    methods.extend(super::domains_task_runtime::constant_methods());
    methods
}
