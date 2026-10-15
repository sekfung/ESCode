//! 契约清单的**服务域**条目（settings/setting/system/file/escode-session/escode-task/v4…）。
//! 与 `inventory.rs` 的 control 条目合成 `host_proto::methods()`；拆文件只为守住单文件行数上限。

use crate::Method;
use crate::State;

/// 服务域方法清单（`inventory.rs` 的 `CONTROL_METHODS` 之后追加）。
pub const SERVICE_METHODS: &[Method] = &[
    Method {
        channel: "settings",
        method: "settingsRead",
        state: State::Implemented,
    },
    Method {
        channel: "settings",
        method: "settingsWrite",
        state: State::Implemented,
    },
    Method {
        channel: "settings",
        method: "settingsValue",
        state: State::Implemented,
    },
    // ---- Renderer RPC 面（@escode/rpc；与 `host-rpc` 的方法表双向对账，见 host-rpc/tests/contract.rs）----
    // setting（优先序 3）
    Method {
        channel: "setting",
        method: "get",
        state: State::Implemented,
    },
    Method {
        channel: "setting",
        method: "update",
        state: State::Implemented,
    },
    Method {
        channel: "setting",
        method: "updateDataBaseDir",
        state: State::Pending,
    },
    Method {
        channel: "setting",
        method: "ensureDefaultProject",
        state: State::Pending,
    },
    // system（优先序 4）
    Method {
        channel: "system",
        method: "info",
        state: State::Implemented,
    },
    Method {
        channel: "system",
        method: "listIntegratedTerminalShells",
        state: State::Pending,
    },
    Method {
        channel: "system",
        method: "probeIntranet",
        state: State::Pending,
    },
    // file（优先序 4）
    Method {
        channel: "file",
        method: "stat",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "checkFilesExist",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "resolvePath",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "readFileRange",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "readTextFile",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "readMediaPreview",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "readBinaryPreview",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "createDefaultWorkspace",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "createScratchWorkspace",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "readdir",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "ensureConversationWorkspace",
        state: State::Implemented,
    },
    Method {
        channel: "file",
        method: "searchWorkspaceFiles",
        state: State::Pending,
    },
    Method {
        channel: "file",
        method: "listWorkspaceFilesRange",
        state: State::Pending,
    },
    Method {
        channel: "file",
        method: "workspaceFileListLength",
        state: State::Pending,
    },
    // escode-session（优先序 5）
    Method {
        channel: "escode-task",
        method: "getTaskMeta",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "listTasks",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "listPinnedTaskIds",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "listPinnedTasks",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "listDeletedTaskIds",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "listArchivedTasks",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "listGroupedTaskViewStructure",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "createTask",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "listTaskList",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "getTaskModelSelection",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "sendPrompt",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "stopGeneration",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "respondPermission",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "setModel",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "setMode",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "closeTask",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "resumeTask",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "archiveWorkspaceTasks",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "getTaskSnapshot",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "getTaskSnapshotWithEtag",
        state: State::Implemented,
    },
    // TS 适配器本身 `return null`（该 Host 不实现），Rust 侧照抄该语义。
    Method {
        channel: "escode-task",
        method: "getTaskSnapshotBody",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "getTaskSnapshotRef",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "getTaskSnapshotToolCallsSlice",
        state: State::Implemented,
    },
    Method {
        channel: "escode-task",
        method: "getTaskConfigOptions",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "listSessions",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "readSession",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "readSessionMessages",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "closeSession",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "setModel",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "setThoughtLevel",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "setMode",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "initializeWorkspace",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "readWorkspacePresentation",
        state: State::Implemented,
    },
    Method {
        channel: "escode-session",
        method: "createSession",
        state: State::Pending,
    },
    Method {
        channel: "escode-session",
        method: "resumeSession",
        state: State::Pending,
    },
    Method {
        channel: "escode-session",
        method: "promoteDeferredDraftSession",
        state: State::Pending,
    },
    Method {
        channel: "escode-session",
        method: "closeDeferredDraftSession",
        state: State::Pending,
    },
    Method {
        channel: "escode-session",
        method: "readSessionEvents",
        state: State::Pending,
    },
    Method {
        channel: "escode-session",
        method: "getWorkspaceRuntimeIdentity",
        state: State::Pending,
    },
    Method {
        channel: "escode-session",
        method: "respondProviderRuntimeHeaders",
        state: State::Pending,
    },
    // workspace（优先序 4）
    Method {
        channel: "workspace",
        method: "workspace/query",
        state: State::Pending,
    },
    // sessions + tasks（优先序 5）
    Method {
        channel: "v4/conversation",
        method: "v4/conversation/subscribe",
        state: State::Pending,
    },
    Method {
        channel: "v4/conversation",
        method: "v4/conversation/resync",
        state: State::Pending,
    },
];
