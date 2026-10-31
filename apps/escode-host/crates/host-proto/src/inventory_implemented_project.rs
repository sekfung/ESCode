//! `project` / `task-groups` / `remote-projects` 的已实现成员（拆出以守住 400 行上限；
//! 与 `IMPLEMENTED` 合并参与契约对账）。

#[rustfmt::skip]
pub const IMPLEMENTED_PROJECT: &[(&str, &str)] = &[
    ("project", "readProjects"),
    ("project", "readTree"),
    ("project", "readDirectory"),
    ("project", "saveProject"),
    ("project", "deleteProject"),
    ("project", "moveTask"),
    ("project", "reorderProjects"),
    ("project", "prepareSession"),
    ("project", "resolveSessionDirectories"),
    ("task-groups", "read"),
    ("task-groups", "save"),
    ("task-groups", "ungroup"),
    ("task-groups", "move"),
    ("task-groups", "admit"),
    ("remote-projects", "readDirectory"),
    ("remote-projects", "saveHost"),
    ("remote-projects", "bindHost"),
    ("remote-projects", "removeHost"),
    ("remote-projects", "connectHost"),
    ("remote-projects", "addEntry"),
    ("remote-projects", "refreshEntry"),
    ("remote-projects", "removeEntry"),
];
