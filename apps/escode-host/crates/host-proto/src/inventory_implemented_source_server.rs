//! `source-server-management` 的已实现成员（拆出以守住 400 行上限；与 `IMPLEMENTED` 合并参与
//! 契约对账，见 docs/specs/source-server-remote-rust-host.md §9/§10/§11）。

#[rustfmt::skip]
pub const IMPLEMENTED_SOURCE_SERVER: &[(&str, &str)] = &[
    ("source-server-management", "abandonWorkspaceMigration"),
    ("source-server-management", "confirmIdentityChange"),
    ("source-server-management", "connectProfile"),
    ("source-server-management", "disconnectSource"),
    ("source-server-management", "ensureConversationWorkspace"),
    ("source-server-management", "exportMigrationReport"),
    ("source-server-management", "getMigrationReport"),
    ("source-server-management", "getRecentOperationLogs"),
    ("source-server-management", "getSnapshot"),
    ("source-server-management", "onDidAppendOperationLog"),
    ("source-server-management", "onDidChangeDirectory"),
    ("source-server-management", "onDidChangeState"),
    ("source-server-management", "removeProfile"),
    ("source-server-management", "removeSourceServer"),
    ("source-server-management", "renameProfile"),
    ("source-server-management", "renameSource"),
    ("source-server-management", "retryWorkspaceMigration"),
    ("source-server-management", "setPreferredProfile"),
    ("source-server-management", "updateProfile"),
    ("source-server-management", "updateSource"),
];
