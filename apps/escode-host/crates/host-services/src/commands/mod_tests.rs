//! `getPrimaryUserCommandsDirectory`（`rust-u2-commands.md` §3/§4）。
use super::*;

#[tokio::test]
async fn creates_the_directory_and_returns_its_verbatim_path() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let expected = home.join(".escode").join("commands");
    assert!(!expected.exists(), "前置：目录还不存在");

    let path = primary_user_commands_directory(home, None).await.unwrap();
    assert_eq!(path, expected.to_string_lossy(), "回包是原样的拼接结果");
    assert!(
        expected.is_dir(),
        "目录必须被创建（Windows 文件管理器打不开不存在的路径）"
    );

    // 幂等：目录已存在时不报错，路径不变。
    let again = primary_user_commands_directory(home, Some("escodeAgent"))
        .await
        .unwrap();
    assert_eq!(again, path);
}

/// 未知来源在**建目录之前**就失败——不能因为来源写错而先在 home 下造出目录。
#[tokio::test]
async fn unknown_source_fails_without_touching_the_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let error = primary_user_commands_directory(dir.path(), Some("nope"))
        .await
        .unwrap_err();
    assert!(error.contains("Unknown command agent source"), "{error}");
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(entries.is_empty(), "失败路径不应留下任何目录: {entries:?}");
}
