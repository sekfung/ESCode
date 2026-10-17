//! `git-checkpoint` 的生命周期与错误文案（`rust-u2-git.md` §8.2/§8.4）。
//! 与主流程用例分文件以守住单文件行数上限；夹具共用 `support::RepoFixture`。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn delete_clears_ref_and_manifest_and_errors_match_ts_wording() {
    let Some(fixture) = RepoFixture::new().await else {
        eprintln!("skip: git 二进制不在场");
        return;
    };
    let workspace = &fixture.workspace;
    let ws = fixture.workspace_path.clone();
    let mut socket = open(&fixture.surface).await;

    // 建两个 checkpoint（删除语义与内容变化无关，故不改文件）。
    let mut ids: Vec<String> = Vec::new();
    for id in [101_i64, 102] {
        call(
            &mut socket,
            id,
            "git-checkpoint",
            "createCheckpoint",
            &params(serde_json::json!({"workspacePath": ws})),
        )
        .await;
        let Wire::Object(meta) = read_response(&mut socket, id).await else {
            panic!("createCheckpoint 应答应为对象")
        };
        ids.push(
            meta["checkpointId"]
                .as_str()
                .expect("checkpointId")
                .to_owned(),
        );
    }
    let (a_id, b_id) = (ids[0].clone(), ids[1].clone());
    assert_ne!(a_id, b_id, "两次 create 必须拿到不同 id");
    assert_eq!(
        git(workspace, &["for-each-ref", "refs/escode/checkpoints"])
            .lines()
            .count(),
        2
    );
    let manifest_dir = fixture.manifest_dir().expect("manifest 目录已创建");

    // 删 A：ref 少一条，A 的 manifest 文件消失（B 的不动）。
    call(
        &mut socket,
        103,
        "git-checkpoint",
        "deleteCheckpoint",
        &params(serde_json::json!({"workspacePath": ws, "checkpointId": a_id})),
    )
    .await;
    assert!(
        read_response(&mut socket, 103)
            .await
            .into_object()
            .is_none(),
        "deleteCheckpoint 回 undefined"
    );
    assert_eq!(
        git(workspace, &["for-each-ref", "refs/escode/checkpoints"])
            .lines()
            .count(),
        1,
        "删除后只剩一条 hidden ref"
    );
    assert!(
        !manifest_dir.join(format!("{a_id}.json")).exists(),
        "A 的 manifest 文件应已删除"
    );
    assert!(
        manifest_dir.join(format!("{b_id}.json")).exists(),
        "B 的 manifest 不受影响"
    );

    // 删过的 checkpoint：后续操作报「不存在」（TS 文案）。
    call(
        &mut socket,
        104,
        "git-checkpoint",
        "diffCheckpoints",
        &params(serde_json::json!({
            "workspacePath": ws,
            "fromCheckpointId": a_id,
            "toCheckpointId": b_id,
        })),
    )
    .await;
    let error = error_of(&mut socket, 104).await;
    assert_eq!(
        error["message"],
        serde_json::json!(format!("Checkpoint does not exist: {a_id}")),
        "{error}"
    );

    // 从未存在过的 id 走同一条错误，错误名是普通 `Error`（TS 抛的就是 `Error`）。
    call(
        &mut socket,
        105,
        "git-checkpoint",
        "deleteCheckpoint",
        &params(serde_json::json!({"workspacePath": ws, "checkpointId": "nope"})),
    )
    .await;
    let error = error_of(&mut socket, 105).await;
    assert_eq!(
        error["message"],
        serde_json::json!("Checkpoint does not exist: nope"),
        "{error}"
    );
    assert_eq!(error["name"], serde_json::json!("Error"));

    // 非仓库工作区：前置校验文案（逐字）。
    let outside = fixture._dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    call(
        &mut socket,
        106,
        "git-checkpoint",
        "createCheckpoint",
        &params(serde_json::json!({"workspacePath": outside.to_string_lossy()})),
    )
    .await;
    let error = error_of(&mut socket, 106).await;
    assert_eq!(
        error["message"],
        serde_json::json!("Workspace is not inside a Git repository."),
        "{error}"
    );

    // 删 B 也必须成功（ref 存在 → 走真删分支）。
    call(
        &mut socket,
        107,
        "git-checkpoint",
        "deleteCheckpoint",
        &params(serde_json::json!({"workspacePath": ws, "checkpointId": b_id})),
    )
    .await;
    assert!(
        read_response(&mut socket, 107)
            .await
            .into_object()
            .is_none()
    );
    assert_eq!(
        git(workspace, &["for-each-ref", "refs/escode/checkpoints"])
            .lines()
            .count(),
        0,
        "两条 ref 都删掉了"
    );
    fixture.assert_temp_index_clean();

    fixture.surface.shutdown().await;
}
