//! `escode-task.restartWorkspaceProcess`：使 workspace 的运行时实例失效，下次使用时懒重启（新世代）。
mod support;
use support::*;

fn params(workspace: &std::path::Path) -> Wire {
    Wire::Array(vec![Wire::Object(serde_json::json!({
        "workspacePath": workspace.to_string_lossy(),
        "workspaceIdentity": workspace.to_string_lossy(),
    }))])
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_closes_the_instance_and_the_next_call_starts_a_new_generation() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let (surface, instances) = start_with_instances(&home, &home).await;
    let mut socket = open(&surface).await;
    let identity = workspace.to_string_lossy().into_owned();

    call(
        &mut socket,
        1,
        "escode-session",
        "listSessions",
        &params(&workspace),
    )
    .await;
    read_response(&mut socket, 1).await;
    let first = instances.generations()[&identity];
    assert!(
        instances.live(&identity).is_some(),
        "listSessions 懒启动了实例"
    );

    call(
        &mut socket,
        2,
        "escode-task",
        "restartWorkspaceProcess",
        &params(&workspace),
    )
    .await;
    assert_eq!(read_response(&mut socket, 2).await, Wire::Undefined);
    assert!(instances.live(&identity).is_none(), "实例已关闭");
    assert_eq!(
        instances.generations()[&identity],
        first,
        "关闭不改世代，世代只在重启时前进"
    );

    call(
        &mut socket,
        3,
        "escode-session",
        "listSessions",
        &params(&workspace),
    )
    .await;
    read_response(&mut socket, 3).await;
    assert!(instances.live(&identity).is_some(), "下一次使用懒重启");
    assert_eq!(instances.generations()[&identity], first + 1, "新世代");

    // 没有实例的 workspace、重复重启：幂等无错。
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    call(
        &mut socket,
        4,
        "escode-task",
        "restartWorkspaceProcess",
        &params(&other),
    )
    .await;
    assert_eq!(read_response(&mut socket, 4).await, Wire::Undefined);
}

#[tokio::test(flavor = "multi_thread")]
async fn release_preparation_closes_the_instance_and_constant_answers_match_ts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let (surface, instances) = start_with_instances(&home, &home).await;
    let mut socket = open(&surface).await;
    let identity = workspace.to_string_lossy().into_owned();

    call(
        &mut socket,
        1,
        "escode-session",
        "listSessions",
        &params(&workspace),
    )
    .await;
    read_response(&mut socket, 1).await;
    assert!(instances.live(&identity).is_some());
    call(
        &mut socket,
        2,
        "escode-task",
        "releaseWorkspacePreparation",
        &params(&workspace),
    )
    .await;
    assert_eq!(read_response(&mut socket, 2).await, Wire::Undefined);
    assert!(
        instances.live(&identity).is_none(),
        "释放预热 = 回收 runtime"
    );

    call(
        &mut socket,
        3,
        "escode-task",
        "getWorkspaceProviderConfigFile",
        &params(&workspace),
    )
    .await;
    let Wire::Object(file) = read_response(&mut socket, 3).await else {
        panic!("object")
    };
    assert_eq!(
        file,
        serde_json::json!({"provider": "glm", "path": identity, "exists": false})
    );

    let task = Wire::Array(vec![Wire::Object(serde_json::json!({
        "workspacePath": "/w", "taskId": "t-1",
    }))]);
    call(
        &mut socket,
        4,
        "escode-task",
        "getTaskSessionFilePath",
        &task,
    )
    .await;
    let Wire::Object(path) = read_response(&mut socket, 4).await else {
        panic!("object")
    };
    assert_eq!(
        path,
        serde_json::json!({"path": "/w/t-1.escode-session", "exists": false})
    );

    call(
        &mut socket,
        5,
        "escode-task",
        "checkCodexConnectivity",
        &Wire::Array(Vec::new()),
    )
    .await;
    let Wire::Object(check) = read_response(&mut socket, 5).await else {
        panic!("object")
    };
    assert_eq!(
        check,
        serde_json::json!({"target": "escode-agent", "reachable": true})
    );
}
