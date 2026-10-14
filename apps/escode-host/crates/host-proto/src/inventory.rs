//! 契约清单本体（channel/method 与实现状态）。新增控制消息或服务方法必须在这里登记。

use crate::Method;
use crate::State;

/// 契约清单（u2a 起点：先登记阶段 2 优先序 3–5 的服务面；迁移一域补一域）。
/// `control` 通道的方法名即 `ControlMessage` 的 serde tag（漂移由 host-proto 的
/// `control_inventory_matches_the_protocol_enum` 测试拦截）。
pub const METHODS: &[Method] = &[
    // 通路（u2a 自带；Process = 进程入口的控制循环已实现）
    Method {
        channel: "control",
        method: "hostReady",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowAttach",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowAttached",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowRequest",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowFrame",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowDetach",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "windowDetached",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "logBatch",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "failed",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "shutdown",
        state: State::Implemented,
    },
    Method {
        channel: "control",
        method: "stopped",
        state: State::Implemented,
    },
    // settings（优先序 3；Rust 侧读/写与 TS Host 同文件同格式，交替写读演练已通过）
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
        method: "createTask",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "listTasks",
        state: State::Pending,
    },
    Method {
        channel: "escode-task",
        method: "listTaskList",
        state: State::Pending,
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
        state: State::Pending,
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
