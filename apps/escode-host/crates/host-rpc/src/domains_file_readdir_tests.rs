#[cfg(test)]
use super::*;

/// 把一条 readdir 结果取成 `(name, type, isSymbolicLink)`（`Wire` 无下标，测试里显式解构）。
fn row(entry: &Wire) -> (String, String, bool) {
    let Wire::Object(object) = entry else {
        panic!("object: {entry:?}")
    };
    (
        object["name"].as_str().unwrap().to_owned(),
        object["type"].as_str().unwrap().to_owned(),
        object["isSymbolicLink"].as_bool().unwrap(),
    )
}

/// `node_join` 对齐 Node `path.join`（期望值由 `node -e 'path.join(a,b)'` 生成）。
#[test]
fn node_join_matches_node_path_join() {
    let cases = [
        ("/a/b/", "x", "/a/b/x"),
        ("/a//b", "x", "/a/b/x"),
        ("/a/./b", "x", "/a/b/x"),
        ("/a/b/../c", "x", "/a/c/x"),
        ("a/b", "x", "a/b/x"),
        ("/", "x", "/x"),
        ("/a/b", "..", "/a"),
        ("/a/b", "", "/a/b"),
    ];
    for (base, name, expected) in cases {
        assert_eq!(node_join(base, name), expected, "{base} + {name}");
    }
}

#[test]
fn readdir_filters_hidden_and_resolves_types() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("visible.txt"), b"x").unwrap();
    std::fs::write(root.join(".hidden"), b"x").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::create_dir(root.join("Zeta")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("sub"), root.join("link-to-dir")).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), root.join("dangling")).unwrap();
    }

    let params = json!({"path": root.to_string_lossy()});
    let Wire::Array(entries) = readdir(&params).expect("readdir") else {
        panic!("array")
    };
    let rows: Vec<(String, String, bool)> = entries.iter().map(row).collect();
    assert!(
        !rows.iter().any(|(name, _, _)| name == ".hidden"),
        "默认过滤隐藏项"
    );

    #[cfg(unix)]
    {
        // 修复：names 只在 Unix 顺序断言里用；放在块外时 Windows 上只剩一次 contains，
        // clippy needless_collect 在 -D warnings 下报错（windows-latest checks 暴露）。
        let names: Vec<String> = rows.iter().map(|(name, _, _)| name.clone()).collect();
        // 目录组在前（含指向目录的软链与目录本身），组内按 localeCompare。
        assert_eq!(names[0], "link-to-dir", "{names:?}");
        assert_eq!(names[1], "sub", "{names:?}");
        assert_eq!(names[2], "Zeta", "{names:?}");
        assert_eq!(names[3], "dangling", "{names:?}");
        assert_eq!(names[4], "visible.txt", "{names:?}");
        let by_name = |name: &str| {
            rows.iter()
                .find(|(candidate, _, _)| candidate == name)
                .unwrap()
                .clone()
        };
        assert_eq!(by_name("link-to-dir").1, "directory");
        assert!(by_name("link-to-dir").2);
        // 悬空软链跟随失败 → file（TS `resolveReaddirEntryType` 的 catch 分支）。
        assert_eq!(by_name("dangling").1, "file");
        assert!(by_name("dangling").2);
        assert_eq!(by_name("sub").1, "directory");
        assert!(!by_name("sub").2);
    }

    // includeHidden：隐藏项在场。
    let Wire::Array(all) = readdir(&json!({
        "path": root.to_string_lossy(),
        "includeHidden": true,
    }))
    .expect("readdir") else {
        panic!("array")
    };
    assert!(
        all.iter().any(|entry| row(entry).0 == ".hidden"),
        "includeHidden 时不丢隐藏项"
    );

    // 目录不存在：Node `scandir` 文案（op 与 TS `fs.readdir` 一致）。
    let error =
        readdir(&json!({"path": root.join("missing").to_string_lossy()})).expect_err("ENOENT");
    assert_eq!(error.0, "NodeIoError");
    assert!(error.1.starts_with("ENOENT:"), "{error:?}");
    assert!(error.1.contains("scandir"), "{error:?}");
}
