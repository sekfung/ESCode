//! `escode-task` 在缺 `dataBaseDir` 时的显式缺口表（从 `domains_task.rs` 拆出）。

use std::sync::Arc;

use crate::session::{Channel, handler};
use crate::wire::Wire;
use crate::workspace_events::WorkspaceEvents;

/// 方法名与有 `dataBaseDir` 时一致（契约对账），调用一律回结构化缺口。
pub(super) fn gap_channel(
    events: &Arc<WorkspaceEvents>,
    task_events: &Arc<crate::task_events::TaskEventHub>,
) -> Channel {
    let gap = "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.";
    let missing = || {
        handler(move |_arg: Wire| {
            let gap = gap.to_owned();
            Box::pin(async move { Err(("InvalidRequest".to_owned(), gap)) })
        })
    };
    Channel::new(
        "escode-task",
        vec![
            ("getTaskMeta", missing()),
            ("listTasks", missing()),
            ("listPinnedTaskIds", missing()),
            ("listPinnedTasks", missing()),
            ("listDeletedTaskIds", missing()),
            ("listArchivedTasks", missing()),
            ("listTaskList", missing()),
            ("listGroupedTaskViewStructure", missing()),
            ("getTaskModelSelection", missing()),
            ("getTaskSnapshot", missing()),
            ("getTaskSnapshotWithEtag", missing()),
            ("getTaskSnapshotBody", missing()),
            ("getTaskSnapshotRef", missing()),
            ("getTaskSnapshotToolCallsSlice", missing()),
            ("getTaskConfigOptions", missing()),
            ("setTaskPinned", missing()),
            ("archiveTask", missing()),
            ("unarchiveTask", missing()),
            ("setTaskUnread", missing()),
            ("renameTask", missing()),
            ("deleteTask", missing()),
            ("deleteArchivedTask", missing()),
            ("deleteArchivedTasks", missing()),
            ("archiveStaleTasks", missing()),
            ("archiveWorkspaceTasks", missing()),
            ("getTaskNativeSessionLogFile", missing()),
            ("getTaskTokenUsage", missing()),
            ("closeTask", missing()),
            ("resumeTask", missing()),
            ("createTask", missing()),
            ("switchAgent", missing()),
            ("initialize", missing()),
            ("goalSession", missing()),
            ("scanImportableClaudeSessions", missing()),
            ("importClaudeSessions", missing()),
            ("syncTaskSessionBinding", missing()),
            ("enqueueTaskCommand", missing()),
            ("promoteTaskCommand", missing()),
            ("cancelTaskCommand", missing()),
            ("sendSessionMessageDeliveryResult", missing()),
            ("deliverSessionMessage", missing()),
            ("branchTaskFromPrompt", missing()),
            ("setAssistantMessageFeedback", missing()),
            ("setAutomationSessionConfig", missing()),
            ("setConfigOption", missing()),
            ("setModel", missing()),
            ("setMode", missing()),
            ("sendPrompt", missing()),
            ("stopGeneration", missing()),
            ("compactSession", missing()),
            ("respondPermission", missing()),
            ("respondElicitation", missing()),
            ("createTaskGroup", missing()),
            ("renameTaskGroup", missing()),
            ("updateTaskGroupColor", missing()),
            ("deleteTaskGroup", missing()),
            ("applyGroupedTaskViewOrder", missing()),
            ("restartWorkspaceProcess", missing()),
            ("releaseWorkspacePreparation", missing()),
            ("getWorkspaceProviderConfigFile", missing()),
            ("getTaskSessionFilePath", missing()),
            ("checkCodexConnectivity", missing()),
        ],
    )
    // 事件只登记监听，不依赖索引：缺 dataBaseDir 时照常可监听（写面都是缺口，不会有事件）。
    .with_events({
        let mut handlers = vec![(
            "onDynamicWorkspaceEvent",
            super::domains_task_state::workspace_event(events),
        )];
        handlers.extend(task_events.handlers());
        handlers
    })
}
