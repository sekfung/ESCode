//! `git-checkpoint` 主流程端到端（真临时 git 仓库 + 真 WebSocket，`rust-u2-git.md` §8.4）。
//!
//! 覆盖：三条最重的链路（snapshot → diff → restore）与 restore 的**三方语义**
//! （磁盘偏离声明基线时不写盘、报冲突，`force` 才恢复），以及本域的核心不变量——
//! 不碰用户真实 index、不产生用户可见分支、临时 index 目录清理干净。
//! 删除与错误文案见 `rpc_git_checkpoint_lifecycle.rs`。
mod support;
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn snapshots_diff_and_the_three_way_restore_guard() {
    let Some(fixture) = RepoFixture::new().await else {
        eprintln!("skip: git 二进制不在场");
        return;
    };
    let workspace = &fixture.workspace;
    let ws = fixture.workspace_path.clone();
    let mut socket = open(&fixture.surface).await;

    // ── 建 checkpoint A ─────────────────────────────────────────────
    call(
        &mut socket,
        91,
        "git-checkpoint",
        "createCheckpoint",
        &params(serde_json::json!({"workspacePath": ws})),
    )
    .await;
    let Wire::Object(a) = read_response(&mut socket, 91).await else {
        panic!("createCheckpoint 应答应为对象")
    };
    let a_id = a["checkpointId"].as_str().expect("checkpointId").to_owned();
    assert_eq!(a["scope"], serde_json::json!("workspace"));
    assert_eq!(a["workspaceInRepoPath"], serde_json::json!("."));
    assert_eq!(a["workspacePath"], serde_json::json!(ws));
    assert_eq!(
        a["commitOid"].as_str().unwrap().len(),
        40,
        "commit oid: {a}"
    );
    // refName 形状（hash 口径由 host-services 的单测钉住）：稍后与 git 里真实的 ref 比对。
    let ref_name = a["refName"].as_str().unwrap().to_owned();
    let parts: Vec<&str> = ref_name.split('/').collect();
    assert_eq!(&parts[..3], ["refs", "escode", "checkpoints"]);
    assert_eq!(
        parts[3].len(),
        12,
        "workspaceHash 是 12 位十六进制: {ref_name}"
    );
    assert_eq!(parts[4], a_id, "ref 末段是 checkpointId");

    assert!(fixture.manifest_root().exists(), "manifest 根应已创建");
    fixture.assert_temp_index_clean();

    // ── 改文件 + 新增文件 → checkpoint B ────────────────────────────
    write(&workspace.join("a.txt"), "two\n");
    write(&workspace.join("b.txt"), "new\n");
    call(
        &mut socket,
        92,
        "git-checkpoint",
        "createCheckpoint",
        &params(serde_json::json!({"workspacePath": ws})),
    )
    .await;
    let Wire::Object(b) = read_response(&mut socket, 92).await else {
        panic!("createCheckpoint 应答应为对象")
    };
    let b_id = b["checkpointId"].as_str().unwrap().to_owned();

    // ── diff A→B：三种路径写法 + kind/added/removed ──────────────────
    call(
        &mut socket,
        93,
        "git-checkpoint",
        "diffCheckpoints",
        &params(serde_json::json!({
            "workspacePath": ws,
            "fromCheckpointId": a_id,
            "toCheckpointId": b_id,
        })),
    )
    .await;
    let Wire::Object(diff) = read_response(&mut socket, 93).await else {
        panic!("diffCheckpoints 应答应为对象")
    };
    assert_eq!(diff["fromCheckpointId"], serde_json::json!(a_id));
    assert_eq!(diff["toCheckpointId"], serde_json::json!(b_id));
    let files = diff["files"].as_array().expect("files 数组");
    assert_eq!(files.len(), 2, "a.txt 改 + b.txt 增: {diff}");
    let modified = files
        .iter()
        .find(|file| file["repoRelativePath"] == serde_json::json!("a.txt"))
        .expect("a.txt 在 diff 里");
    let added = files
        .iter()
        .find(|file| file["repoRelativePath"] == serde_json::json!("b.txt"))
        .expect("b.txt 在 diff 里");
    assert_eq!(modified["kind"], serde_json::json!("modified"));
    assert_eq!(modified["added"], serde_json::json!(1));
    assert_eq!(modified["removed"], serde_json::json!(1));
    assert_eq!(
        modified["workspaceRelativePath"],
        serde_json::json!("a.txt")
    );
    assert_eq!(
        modified["path"],
        serde_json::json!(workspace.join("a.txt").to_string_lossy()),
        "path 是绝对路径"
    );
    assert_eq!(
        modified["originalPath"],
        serde_json::json!(null),
        "无 rename 时 originalPath 恒在且为 null：{modified}"
    );
    assert_eq!(added["kind"], serde_json::json!("added"));

    // ── 把受影响文件改脏 → restore 必须拒绝并报冲突（不写盘）──────────
    // 方向：`from` = 调用方**声明磁盘当前所在**的状态（B），`to` = 想恢复到的目标（A）。
    // 「撤销 A→B 之间的工作」就是这个方向。
    let restore_to_a = |force: bool| {
        let mut value = serde_json::json!({
            "workspacePath": ws,
            "fromCheckpointId": b_id,
            "toCheckpointId": a_id,
        });
        if force {
            value["force"] = serde_json::json!(true);
        }
        params(value)
    };
    write(&workspace.join("a.txt"), "dirty\n");
    call(
        &mut socket,
        94,
        "git-checkpoint",
        "restoreBetweenCheckpoints",
        &restore_to_a(false),
    )
    .await;
    let Wire::Object(refused) = read_response(&mut socket, 94).await else {
        panic!("restore 应答应为对象")
    };
    assert_eq!(refused["success"], serde_json::json!(false), "{refused}");
    assert_eq!(
        refused.get("restoredPaths"),
        None,
        "失败分支不带 restoredPaths"
    );
    let conflicts = refused["conflicts"].as_array().expect("conflicts 数组");
    // 只有 a.txt 偏离 B；b.txt 与 B 一致，不算冲突——被触达但**没偏离**的路径不该报冲突。
    assert_eq!(conflicts.len(), 1, "只应报 a.txt 一条: {refused}");
    assert_eq!(conflicts[0]["repoRelativePath"], serde_json::json!("a.txt"));
    assert_eq!(
        conflicts[0]["reason"],
        serde_json::json!("content-mismatch")
    );
    assert_eq!(
        conflicts[0]["path"],
        serde_json::json!(workspace.join("a.txt").to_string_lossy()),
        "冲突带绝对路径"
    );
    assert_eq!(
        read(&workspace.join("a.txt")),
        "dirty\n",
        "被挡住时**不许**写盘"
    );
    assert_eq!(
        read(&workspace.join("b.txt")),
        "new\n",
        "未受影响的文件也不动"
    );

    // ── force 恢复：磁盘回到 A 基线（含删除 B 新增的 b.txt）────────────
    call(
        &mut socket,
        95,
        "git-checkpoint",
        "restoreBetweenCheckpoints",
        &restore_to_a(true),
    )
    .await;
    let Wire::Object(restored) = read_response(&mut socket, 95).await else {
        panic!("restore 应答应为对象")
    };
    assert_eq!(restored["success"], serde_json::json!(true), "{restored}");
    assert_eq!(
        read(&workspace.join("a.txt")),
        "one\n",
        "force 后回到 to 基线"
    );
    assert!(
        !workspace.join("b.txt").exists(),
        "B 里 added 的文件在回到 A 时应被删掉（diff 的 deleted 分支）"
    );
    assert_eq!(
        restored["restoredPaths"].as_array().unwrap().len(),
        2,
        "restoredPaths 含 deleted 的绝对路径: {restored}"
    );

    // ── 反向恢复 A→B：磁盘正好在 A 基线，应无冲突地重放 B ──────────────
    // 同时覆盖冲突检测的两个「跳过」分支：b.txt 在 A 的 tree 里没有、磁盘上也没有
    // （不该报 unexpected-file-in-worktree），以及 a.txt 内容等于 A（不该报 content-mismatch）。
    call(
        &mut socket,
        96,
        "git-checkpoint",
        "restoreBetweenCheckpoints",
        &params(serde_json::json!({
            "workspacePath": ws,
            "fromCheckpointId": a_id,
            "toCheckpointId": b_id,
        })),
    )
    .await;
    let Wire::Object(replayed) = read_response(&mut socket, 96).await else {
        panic!("restore 应答应为对象")
    };
    assert_eq!(replayed["success"], serde_json::json!(true), "{replayed}");
    assert_eq!(read(&workspace.join("a.txt")), "two\n");
    assert_eq!(read(&workspace.join("b.txt")), "new\n", "b.txt 被重新恢复");

    // ── 不变量：用户 index 未被动过、无新分支、hidden ref 恰两条 ─────
    assert_eq!(
        git(workspace, &["diff", "--cached", "--name-only"]).trim(),
        "",
        "真实 index（staged 区）必须为空——checkpoint 不碰用户暂存区"
    );
    let branches = git(
        workspace,
        &["branch", "--list", "--format=%(refname:short)"],
    );
    assert_eq!(
        branches
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        1,
        "不得产生用户可见分支: {branches}"
    );
    let refs = git(
        workspace,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/escode/checkpoints",
        ],
    );
    assert_eq!(
        refs.lines().count(),
        2,
        "两个 checkpoint 各挂一条 hidden ref"
    );
    assert!(
        refs.lines().any(|line| line == ref_name),
        "应答里的 refName 必须就是 git 里真实存在的那条: {refs}"
    );
    fixture.assert_temp_index_clean();

    fixture.surface.shutdown().await;
}
