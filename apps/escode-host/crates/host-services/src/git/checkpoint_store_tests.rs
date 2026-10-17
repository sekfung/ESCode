//! checkpoint manifest 存储（`rust-u2-git.md` §8.1）：hash 口径、落盘格式、读校验的 fail-closed。
use super::*;

/// `workspaceHash` = `sha256(workspacePath)` 前 12 位——**不带 identity**。
/// 期望值是外部算的（`sha256("/tmp/ws")`），不是从实现反推的：
/// 若哪天有人"顺手统一"成 sessions 目录的 `workspaceKey` 口径，这里立即失败。
#[test]
fn workspace_hash_is_sha256_of_the_path_without_identity() {
    assert_eq!(workspace_hash("/tmp/ws"), "b011ea26cc73");
    assert_eq!(
        workspace_hash("/home/sekfung/Projects/ESCode"),
        "c0d4b406d080"
    );
    assert_eq!(workspace_hash("/tmp/ws").len(), 12);
    // 同一路径恒等；不同路径不同。
    assert_eq!(workspace_hash("/tmp/ws"), workspace_hash("/tmp/ws"));
    assert_ne!(workspace_hash("/tmp/ws"), workspace_hash("/tmp/ws/"));
}

/// 落盘格式 = `JSON.stringify(meta, null, 2) + "\n"`：2 空格缩进、键序为 TS 字面量序、结尾一个换行。
/// 键序与缩进都是**跨 Host 互读**的契约（TS Host 与 Rust Host 轮流读写同一份 manifest）。
#[tokio::test]
async fn saved_manifest_is_byte_identical_to_the_ts_layout() {
    let dir = tempfile::tempdir().unwrap();
    let store = GitCheckpointStore::new(checkpoints_dir(dir.path()));
    let meta = GitCheckpointMeta {
        checkpoint_id: "cp-1".to_owned(),
        workspace_path: "/tmp/ws".to_owned(),
        repo_root: "/tmp/ws".to_owned(),
        workspace_in_repo_path: ".".to_owned(),
        created_at: serde_json::Number::from(1712345678901_i64),
        ref_name: "refs/escode/checkpoints/b011ea26cc73/cp-1".to_owned(),
        commit_oid: "0".repeat(40),
        scope: "workspace".to_owned(),
    };
    store.save(&meta).await.unwrap();

    let path = checkpoints_dir(dir.path())
        .join("b011ea26cc73")
        .join("cp-1.json");
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let expected = format!(
        "{{\n  \"checkpointId\": \"cp-1\",\n  \"workspacePath\": \"/tmp/ws\",\n  \"repoRoot\": \"/tmp/ws\",\n  \"workspaceInRepoPath\": \".\",\n  \"createdAt\": 1712345678901,\n  \"refName\": \"refs/escode/checkpoints/b011ea26cc73/cp-1\",\n  \"commitOid\": \"{}\",\n  \"scope\": \"workspace\"\n}}\n",
        "0".repeat(40)
    );
    assert_eq!(raw, expected);
    // 回读必须拿到同一份（往返无损，含 createdAt 的整数写法）。
    assert_eq!(store.load("/tmp/ws", "cp-1").await.unwrap(), meta);
}

/// 读校验 fail-closed：不存在、坏 JSON、缺字段、类型不符、`scope` 非 `workspace` 一律 `None`
/// （损坏的 manifest 与"不存在"对调用方同义，TS 同）。
#[tokio::test]
async fn load_rejects_anything_that_is_not_a_workspace_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let store = GitCheckpointStore::new(checkpoints_dir(dir.path()));
    let file = checkpoints_dir(dir.path())
        .join("b011ea26cc73")
        .join("cp-1.json");
    tokio::fs::create_dir_all(file.parent().unwrap())
        .await
        .unwrap();

    let valid = r#"{"checkpointId":"cp-1","workspacePath":"/tmp/ws","repoRoot":"/tmp/ws","workspaceInRepoPath":".","createdAt":1,"refName":"r","commitOid":"c","scope":"workspace"}"#;
    let cases: [(&str, &str); 6] = [
        ("坏 JSON", "{not json"),
        ("不是对象", "[]"),
        ("缺字段", r#"{"checkpointId":"cp-1","scope":"workspace"}"#),
        (
            "类型不符",
            r#"{"checkpointId":1,"workspacePath":"/tmp/ws","repoRoot":"/tmp/ws","workspaceInRepoPath":".","createdAt":1,"refName":"r","commitOid":"c","scope":"workspace"}"#,
        ),
        (
            "scope 非 workspace",
            r#"{"checkpointId":"cp-1","workspacePath":"/tmp/ws","repoRoot":"/tmp/ws","workspaceInRepoPath":".","createdAt":1,"refName":"r","commitOid":"c","scope":"repo"}"#,
        ),
        (
            "createdAt 非数字",
            r#"{"checkpointId":"cp-1","workspacePath":"/tmp/ws","repoRoot":"/tmp/ws","workspaceInRepoPath":".","createdAt":"1","refName":"r","commitOid":"c","scope":"workspace"}"#,
        ),
    ];
    for (label, content) in cases {
        tokio::fs::write(&file, content).await.unwrap();
        assert!(
            store.load("/tmp/ws", "cp-1").await.is_none(),
            "{label} 必须被判为无效"
        );
    }
    // 正样本：多出的未知字段不判无效（TS 的浅校验允许）。
    tokio::fs::write(&file, valid.replace("}", ",\"extra\":1}"))
        .await
        .unwrap();
    assert!(
        store.load("/tmp/ws", "cp-1").await.is_some(),
        "未知字段不该判无效"
    );
    // 不存在的文件同样回 None，而不是报错。
    assert!(store.load("/tmp/ws", "missing").await.is_none());
}

#[tokio::test]
async fn delete_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = GitCheckpointStore::new(checkpoints_dir(dir.path()));
    // 目录都不存在也当成功（TS `rm -f` 语义）。
    store.delete("/tmp/ws", "absent").await.unwrap();
    store.delete("/tmp/ws", "absent").await.unwrap();
}
