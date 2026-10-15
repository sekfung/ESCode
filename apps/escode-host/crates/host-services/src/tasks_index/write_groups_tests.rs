//! `write_groups` 的语义边界（对齐 TS `TaskIndexRepo` 分组写；跨实现 oracle 见 `rustHostTaskGroups.test.ts`）。
use super::test_support::{expected_ledger, fixture};
use super::write_groups::{GroupInput, OrderInput, TaskRefInput, TopNode, read_members};
use super::*;
use serde_json::json;

fn open() -> (tempfile::TempDir, TasksIndex) {
    let dir = fixture(&expected_ledger());
    let index = TasksIndex::open(&dir.path().join("tasks-index.sqlite")).unwrap();
    for id in ["t1", "t2", "t3"] {
        let meta = json!({"taskId": id, "traceId": format!("escode-{id}"), "title": id, "workspacePath": "/w",
            "createdAt": 10, "updatedAt": 100, "mode": "build"});
        index
            .sync_task_meta(meta.as_object().unwrap(), &SyncOptions::default())
            .unwrap();
    }
    (dir, index)
}

fn task(id: &str) -> TaskRefInput {
    TaskRefInput {
        workspace_path: "/w".into(),
        workspace_identity: None,
        task_id: id.into(),
    }
}

const SCOPES: &[(&str, Option<&str>)] = &[("/w", None)];

fn orders(index: &TasksIndex) -> Vec<(String, String, i64)> {
    let mut statement = index
        .connection_for_tests()
        .prepare("SELECT node_type, node_key, sort_order FROM task_group_view_node_orders ORDER BY sort_order")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn new_groups_go_to_the_top_of_the_order_and_names_are_normalised() {
    let (_dir, index) = open();
    let first = index
        .create_task_group("g1", Some("  Alpha "), Some("red"), 1)
        .unwrap();
    assert_eq!(
        first,
        json!({"id": "g1", "title": "Alpha", "color": "red", "createdAt": 1, "updatedAt": 1})
    );
    let second = index.create_task_group("g2", Some("   "), None, 2).unwrap();
    assert_eq!(second["title"], "New Group");
    assert_eq!(second["color"], "gray");
    // 第一个 group：MIN 缺省 2000 → 1000；第二个插到更小的位置。
    assert_eq!(
        orders(&index),
        vec![
            ("group".into(), "g2".into(), 0),
            ("group".into(), "g1".into(), 1000)
        ]
    );
}

#[test]
fn rename_and_recolor_validate_and_report_missing_groups() {
    let (_dir, index) = open();
    index.create_task_group("g1", None, None, 1).unwrap();
    let renamed = index.rename_task_group("g1", "  ", 5).unwrap();
    assert_eq!(
        (renamed["title"].as_str(), renamed["updatedAt"].as_i64()),
        (Some("New Group"), Some(5))
    );
    assert_eq!(
        index.update_task_group_color("g1", "purple", 6).unwrap()["color"],
        "purple"
    );
    assert_eq!(
        index
            .update_task_group_color("g1", "pink", 7)
            .unwrap_err()
            .to_string(),
        "Task group 颜色无效"
    );
    assert_eq!(
        index
            .rename_task_group("nope", "x", 8)
            .unwrap_err()
            .to_string(),
        "Task group 不存在，无法重命名"
    );
    assert_eq!(
        index
            .update_task_group_color("nope", "red", 8)
            .unwrap_err()
            .to_string(),
        "Task group 不存在，无法更新颜色"
    );
}

#[test]
fn deleting_a_group_also_drops_its_top_level_order_atomically() {
    let (_dir, index) = open();
    index.create_task_group("g1", None, None, 1).unwrap();
    index.delete_task_group("g1").unwrap();
    assert!(orders(&index).is_empty());
    assert_eq!(
        index.delete_task_group("g1").unwrap_err().to_string(),
        "Task group 不存在，无法删除"
    );
}

fn apply(
    index: &TasksIndex,
    top: Vec<TopNode>,
    groups: Vec<GroupInput>,
    provider: Option<&str>,
) -> Result<(), TasksIndexError> {
    index.apply_grouped_task_view_order(
        &OrderInput {
            workspace_scopes: SCOPES,
            top_level_nodes: top,
            groups,
            provider,
        },
        50,
    )
}

#[test]
fn apply_order_persists_membership_and_a_full_top_level_order() {
    let (_dir, index) = open();
    index.create_task_group("g1", None, None, 1).unwrap();
    apply(
        &index,
        vec![TopNode::Task(task("t3")), TopNode::Group("g1".into())],
        vec![GroupInput {
            group_id: "g1".into(),
            task_refs: vec![task("t2"), task("t1")],
        }],
        None,
    )
    .unwrap();
    let members = read_members(index.connection_for_tests()).unwrap();
    assert_eq!(
        members["g1"],
        vec![
            ("/w".to_owned(), "t2".to_owned()),
            ("/w".to_owned(), "t1".to_owned())
        ]
    );
    assert_eq!(
        orders(&index),
        vec![
            ("task".into(), "[\"/w\",\"t3\"]".into(), 1000),
            ("group".into(), "g1".into(), 2000),
        ]
    );
    // 把 t1 提到顶层：退出 group。
    apply(
        &index,
        vec![TopNode::Task(task("t1")), TopNode::Group("g1".into())],
        vec![],
        None,
    )
    .unwrap();
    let members = read_members(index.connection_for_tests()).unwrap();
    assert_eq!(members["g1"], vec![("/w".to_owned(), "t2".to_owned())]);
}

#[test]
fn apply_order_rejects_invalid_input_without_partial_writes() {
    let (_dir, index) = open();
    index.create_task_group("g1", None, None, 1).unwrap();
    index.create_task_group("g2", None, None, 2).unwrap();
    let before = orders(&index);
    let both = vec![
        GroupInput {
            group_id: "g1".into(),
            task_refs: vec![task("t1")],
        },
        GroupInput {
            group_id: "g2".into(),
            task_refs: vec![task("t1")],
        },
    ];
    assert_eq!(
        apply(&index, vec![], both, None).unwrap_err().to_string(),
        "Grouped task order 不能让同一个 task 进入多个 group"
    );
    assert_eq!(
        apply(&index, vec![TopNode::Group("ghost".into())], vec![], None)
            .unwrap_err()
            .to_string(),
        "Grouped task order 包含不存在的 group"
    );
    let outside = TaskRefInput {
        workspace_path: "/other".into(),
        workspace_identity: None,
        task_id: "t1".into(),
    };
    assert_eq!(
        apply(&index, vec![TopNode::Task(outside)], vec![], None)
            .unwrap_err()
            .to_string(),
        "Grouped task order 包含当前 scope 外的 task"
    );
    assert_eq!(
        apply(&index, vec![TopNode::Task(task("missing"))], vec![], None)
            .unwrap_err()
            .to_string(),
        "Grouped task order 包含不可见 task"
    );
    index
        .update_task_state(
            &write_state::TaskRef {
                workspace_path: "/w",
                workspace_identity: None,
                task_id: "t2",
            },
            &write_state::StatePatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        apply(&index, vec![TopNode::Task(task("t2"))], vec![], None)
            .unwrap_err()
            .to_string(),
        "Grouped task order 包含不可见 task"
    );
    assert_eq!(orders(&index), before, "校验失败不改任何状态");
}

#[test]
fn apply_order_skips_tasks_of_other_providers_when_a_provider_is_given() {
    let (_dir, index) = open();
    index.create_task_group("g1", None, None, 1).unwrap();
    index
        .connection_for_tests()
        .execute(
            "UPDATE tasks SET provider = CASE task_id WHEN 't2' THEN 'legacy' ELSE 'glm' END",
            [],
        )
        .unwrap();
    apply(
        &index,
        vec![TopNode::Task(task("t2")), TopNode::Group("g1".into())],
        vec![GroupInput {
            group_id: "g1".into(),
            task_refs: vec![task("t2"), task("t1")],
        }],
        Some("glm"),
    )
    .unwrap();
    let members = read_members(index.connection_for_tests()).unwrap();
    assert_eq!(
        members["g1"],
        vec![("/w".to_owned(), "t1".to_owned())],
        "旧 provider 的引用静默跳过"
    );
    assert_eq!(orders(&index), vec![("group".into(), "g1".into(), 1000)]);
}
