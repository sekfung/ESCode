//! 索引服务单测：缓存（TTL/LRU/签名）、刷新、分块长度语义、规则读写。

use super::*;

fn service() -> FileIndexService {
    FileIndexService::new(Arc::new(|a: &str, b: &str| a.cmp(b)))
}

async fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    tokio::fs::create_dir_all(path.parent().expect("parent"))
        .await
        .expect("mkdir");
    tokio::fs::write(path, content).await.expect("write");
}

fn root_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().into_owned()
}

#[tokio::test]
async fn indexes_filters_and_searches() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "src/main.rs", "").await;
    write(dir.path(), "src/lib.rs", "").await;
    write(dir.path(), ".env", "").await;
    write(dir.path(), ".github/workflow.yml", "").await;
    write(dir.path(), "node_modules/x/index.js", "").await;
    write(dir.path(), "bin/tool.exe", "").await;
    write(dir.path(), ".gitignore", "*.log\n").await;
    write(dir.path(), "a.log", "").await;
    let svc = service();
    let root = root_str(&dir);
    let all = svc
        .search(&root, None, "", None, false)
        .await
        .expect("search");
    let rels: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["relativePath"].as_str().unwrap())
        .collect();
    // 目录排后；.gitignore 命中 `*.log`；node_modules 被默认模板排除；隐藏目录里的文件仍在索引。
    assert_eq!(
        rels,
        vec![
            ".github/workflow.yml",
            ".gitignore",
            "src/lib.rs",
            "src/main.rs",
            "bin",
            "src"
        ]
    );
    let hit = svc
        .search(&root, None, "main", Some(5.0), false)
        .await
        .expect("search");
    assert_eq!(hit[0]["relativePath"], "src/main.rs");
    // 修复：绝对路径按平台分隔符拼接（Windows 上 `{root}/src/main.rs` 与回包的 `\` 分隔不一致）。
    assert_eq!(
        hit[0]["path"],
        std::path::Path::new(&root)
            .join("src")
            .join("main.rs")
            .to_string_lossy()
            .as_ref()
    );
}

#[tokio::test]
async fn limit_and_parameter_validation() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "a.txt", "").await;
    let svc = service();
    let root = root_str(&dir);
    assert_eq!(
        svc.search(&root, None, "a", Some(0.0), false)
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        svc.search(&root, None, "a", Some(-3.0), false)
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        svc.search(&root, None, "a", Some(1.9), false)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let error = svc
        .search(&root, None, "a", Some(f64::NAN), false)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Invalid workspace file search query or limit"
    );
}

#[tokio::test]
async fn cache_serves_stale_until_refresh_or_rule_change() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "a.txt", "").await;
    let svc = service();
    let root = root_str(&dir);
    assert_eq!(
        svc.search(&root, None, "", None, false)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    write(dir.path(), "b.txt", "").await;
    assert_eq!(
        svc.search(&root, None, "", None, false)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1,
        "TTL 内同签名命中缓存"
    );
    assert_eq!(
        svc.search(&root, None, "", None, true)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    write(dir.path(), ".escodeignore", "a.txt\n").await;
    let after_rules = svc.search(&root, None, "", None, false).await.unwrap();
    let rels: Vec<&str> = after_rules
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["relativePath"].as_str().unwrap())
        .collect();
    assert_eq!(rels, vec!["b.txt"], "规则文件变化改变签名，缓存失效");
}

#[tokio::test]
async fn length_and_range_use_utf16_units() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "😀.txt", "").await;
    write(dir.path(), "中.txt", "").await;
    let svc = service();
    let root = root_str(&dir);
    let packed = "file\t中.txt\nfile\t😀.txt";
    assert_eq!(
        svc.length(&root).await.unwrap(),
        packed.encode_utf16().count()
    );
    assert_eq!(svc.range(&root, 0.0, 1e9).await.unwrap(), packed);
    assert_eq!(svc.range(&root, 5.0, 3.0).await.unwrap(), "中.t");
    assert_eq!(svc.range(&root, 1e6, 3.0).await.unwrap(), "");
    assert_eq!(svc.range(&root, -4.0, 4.0).await.unwrap(), "file");
}

#[tokio::test]
async fn ignore_settings_round_trip() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), ".gitignore", "dist/\n").await;
    let svc = service();
    let root = root_str(&dir);
    let preview = svc.read_ignore(&root).await.unwrap();
    assert_eq!(preview["source"], "template");
    let content = preview["content"].as_str().unwrap().to_owned();
    assert!(content.starts_with("dist/\n"));
    assert!(
        !tokio::fs::try_exists(dir.path().join(".escodeignore"))
            .await
            .unwrap(),
        "读取不落盘"
    );
    svc.write_ignore(&root, &format!("{content}my-custom/\n"))
        .await
        .unwrap();
    assert_eq!(svc.read_ignore(&root).await.unwrap()["source"], "file");
    write(dir.path(), ".gitignore", "build/\n").await;
    let synced = svc.transform_ignore(&root, "sync-gitignore").await.unwrap();
    let text = synced["content"].as_str().unwrap();
    assert!(text.starts_with("build/\n"));
    assert!(text.contains("my-custom/"), "自定义区保留");
}
