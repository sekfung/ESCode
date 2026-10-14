#[cfg(test)]
use super::*;
use crate::domains_file_preview::{node_extname, read_binary_preview, read_media_preview};
use crate::domains_file_workspace::{
    create_default_workspace, create_scratch_workspace, ensure_conversation_workspace,
};

#[test]
fn cache_expires_and_evicts_oldest() {
    let mut cache = Cache::default();
    let now = SystemTime::now();
    cache.set("a".into(), true, now);
    assert_eq!(cache.get("a", now), Some(true));
    assert_eq!(
        cache.get("a", now + FILE_EXISTENCE_CACHE_TTL + Duration::from_secs(1)),
        None,
        "过期项不再命中"
    );
    for index in 0..(FILE_EXISTENCE_CACHE_MAX_ENTRIES + 10) {
        cache.set(format!("p{index}"), true, now);
    }
    assert_eq!(cache.len(), FILE_EXISTENCE_CACHE_MAX_ENTRIES);
}

#[test]
fn stat_reports_directory_and_file_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"hello").unwrap();

    let dir_result = stat(&json!({"path": dir.path().to_string_lossy()})).expect("dir stat");
    let Wire::Object(dir_object) = dir_result else {
        panic!("object")
    };
    assert_eq!(dir_object["type"], "directory");
    assert!(
        dir_object.get("size").is_none(),
        "目录不带 size: {dir_object}"
    );

    let file_result = stat(&json!({"path": file.to_string_lossy()})).expect("file stat");
    let Wire::Object(file_object) = file_result else {
        panic!("object")
    };
    assert_eq!(file_object["type"], "file");
    assert_eq!(file_object["size"], 5);
    assert!(file_object["mtimeMs"].as_f64().unwrap_or_default() > 0.0);
}

#[test]
fn stat_missing_path_uses_node_error_shape() {
    let error = stat(&json!({"path": "/definitely/missing/path"})).expect_err("must fail");
    assert_eq!(error.0, "NodeIoError");
    assert_eq!(
        error.1,
        "ENOENT: no such file or directory, stat '/definitely/missing/path'"
    );
}

#[test]
fn existence_check_is_ordered_and_counts_only_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"x").unwrap();
    let missing = dir.path().join("missing.txt");
    let cache = std::sync::Mutex::new(Cache::default());
    let params = json!({"paths": [file.to_string_lossy(), dir.path().to_string_lossy(), missing.to_string_lossy()]});
    let Wire::Object(result) = check_files_exist(&cache, &params).expect("check") else {
        panic!("array")
    };
    let rows = result.as_array().expect("array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["exists"], true, "文件为 true");
    assert_eq!(rows[1]["exists"], false, "目录按 false");
    assert_eq!(rows[2]["exists"], false);
    assert_eq!(rows[0]["path"], file.to_string_lossy().as_ref());

    let too_many: Vec<String> = (0..FILE_EXISTENCE_BATCH_LIMIT + 1)
        .map(|index| format!("/tmp/{index}"))
        .collect();
    let error = check_files_exist(&cache, &json!({"paths": too_many})).expect_err("limit");
    assert_eq!(error.1, "File existence check supports at most 15 paths.");
}

#[test]
fn read_file_range_clamps_offsets_and_lengths() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.bin");
    std::fs::write(&file, vec![7_u8; 4096]).unwrap();
    let path = file.to_string_lossy().into_owned();

    let Wire::Bytes { vs, data } =
        read_file_range(&json!({"path": path, "offset": 0, "length": 10})).expect("read")
    else {
        panic!("bytes")
    };
    assert!(!vs, "顶层 Uint8Array 用 Buffer 标签");
    assert_eq!(data.len(), 10);
    assert!(data.iter().all(|byte| *byte == 7));

    // 负 offset 视作 0。
    let Wire::Bytes { data, .. } =
        read_file_range(&json!({"path": path, "offset": -5, "length": 1})).expect("read")
    else {
        panic!("bytes")
    };
    assert_eq!(data.len(), 1);
    // 超 EOF：空数组。
    let Wire::Bytes { data, .. } =
        read_file_range(&json!({"path": path, "offset": 9999, "length": 10})).expect("read")
    else {
        panic!("bytes")
    };
    assert!(data.is_empty());
    // 长度超上限被 clamp 到 1 MiB（这里文件较小，返回剩余字节）。
    let Wire::Bytes { data, .. } =
        read_file_range(&json!({"path": path, "offset": 0, "length": 10 * 1024 * 1024}))
            .expect("read")
    else {
        panic!("bytes")
    };
    assert_eq!(data.len(), 4096);
    // 目录：报非文件。
    let error = read_file_range(&json!({"path": dir.path().to_string_lossy()})).expect_err("dir");
    assert!(error.1.starts_with("Path is not a file: "), "{error:?}");
}

#[test]
fn read_text_file_slices_and_detects_binary() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("a.txt");
    std::fs::write(&text, "hello world").unwrap();
    let path = text.to_string_lossy().into_owned();

    let Wire::Object(slice) = read_text_file(&json!({"path": path})).expect("read") else {
        panic!("object")
    };
    assert_eq!(slice["content"], "hello world");
    assert_eq!(slice["offset"], 0);
    assert_eq!(slice["bytesRead"], 11);
    assert_eq!(slice["totalBytes"], 11);
    assert_eq!(slice["truncated"], false);
    assert_eq!(slice["isBinary"], false);

    // offset 超过 EOF：空内容 + truncated=false。
    let Wire::Object(slice) = read_text_file(&json!({"path": path, "offset": 99})).expect("read")
    else {
        panic!("object")
    };
    assert_eq!(slice["content"], "");
    assert_eq!(slice["bytesRead"], 0);
    assert_eq!(slice["truncated"], false);

    // 局部读取：offset=6 length=5 → "world"；offset+bytesRead < total → truncated=true。
    let Wire::Object(slice) =
        read_text_file(&json!({"path": path, "offset": 6, "length": 5})).expect("read")
    else {
        panic!("object")
    };
    assert_eq!(slice["content"], "world");
    assert_eq!(
        slice["truncated"], false,
        "读到 EOF 不算截断（TS: offset + bytesRead < totalBytes）"
    );

    // 未读到 EOF：offset=0 length=5 → truncated=true。
    let Wire::Object(slice) =
        read_text_file(&json!({"path": path, "offset": 0, "length": 5})).expect("read")
    else {
        panic!("object")
    };
    assert_eq!(slice["content"], "hello");
    assert_eq!(slice["bytesRead"], 5);
    assert_eq!(slice["truncated"], true);

    // 二进制：含 NUL 即判二进制且内容为空。
    let binary = dir.path().join("b.bin");
    std::fs::write(&binary, [1_u8, 0, 2, 3, 4, 5]).unwrap();
    let Wire::Object(slice) =
        read_text_file(&json!({"path": binary.to_string_lossy()})).expect("read")
    else {
        panic!("object")
    };
    assert_eq!(slice["isBinary"], true);
    assert_eq!(slice["content"], "");

    // 非文件：报错。
    let error = read_text_file(&json!({"path": dir.path().to_string_lossy()})).expect_err("dir");
    assert!(error.1.starts_with("Path is not a file: "), "{error:?}");
}

#[test]
fn binary_detection_follows_the_ts_threshold() {
    // 控制字符占比恰好 0.3 时不算二进制（TS 用严格大于）。
    let mut buffer = vec![b'a'; 7];
    buffer.extend_from_slice(&[1_u8, 2, 3]);
    assert!(!is_probably_binary(&buffer), "0.3 不触发");
    let mut buffer = vec![b'a'; 6];
    buffer.extend_from_slice(&[1_u8, 2, 3, 4]);
    assert!(is_probably_binary(&buffer), "0.4 触发");
    assert!(!is_probably_binary(&[]), "空缓冲不算二进制");
    assert!(
        !is_probably_binary(b"tab\tcr\rlf\nform\x0c"),
        "常见空白不算可疑"
    );
}

#[test]
fn previews_round_trip_base64_and_resolve_media_type() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("pixel.PNG");
    std::fs::write(&image, [1_u8, 2, 3, 4]).unwrap();
    let Wire::Object(preview) =
        read_media_preview(&json!({"path": image.to_string_lossy()})).expect("preview")
    else {
        panic!("object")
    };
    assert_eq!(preview["mediaType"], "image/png", "扩展名大小写不敏感");
    assert_eq!(preview["dataBase64"], "AQIDBA==");
    assert_eq!(preview["totalBytes"], 4);

    // 非图片走共享媒体格式表（endsWith），其余兜底 octet-stream。
    let video = dir.path().join("clip.MP4");
    std::fs::write(&video, [9_u8]).unwrap();
    let Wire::Object(preview) =
        read_media_preview(&json!({"path": video.to_string_lossy()})).expect("preview")
    else {
        panic!("object")
    };
    assert_eq!(preview["mediaType"], "video/mp4");
    let other = dir.path().join("notes.txt");
    std::fs::write(&other, b"hi").unwrap();
    let Wire::Object(preview) =
        read_media_preview(&json!({"path": other.to_string_lossy()})).expect("preview")
    else {
        panic!("object")
    };
    assert_eq!(preview["mediaType"], "application/octet-stream");

    // 二进制预览：完整 base64，无 mediaType 字段。
    let Wire::Object(binary) =
        read_binary_preview(&json!({"path": other.to_string_lossy()})).expect("preview")
    else {
        panic!("object")
    };
    assert_eq!(binary["dataBase64"], "aGk=");
    assert!(binary.get("mediaType").is_none());

    // 超过上限：报 too large（maxBytes 压低到 2 字节）。
    let error = read_media_preview(&json!({"path": image.to_string_lossy(), "maxBytes": 2}))
        .expect_err("too large");
    assert!(
        error.1.starts_with("File is too large to preview: "),
        "{error:?}"
    );
    // 非文件：Path is not a file。
    let error =
        read_binary_preview(&json!({"path": dir.path().to_string_lossy()})).expect_err("dir");
    assert!(error.1.starts_with("Path is not a file: "), "{error:?}");
}

#[test]
fn extname_matches_node_semantics() {
    assert_eq!(node_extname("/a/b/c.PNG"), ".PNG");
    assert_eq!(node_extname("/a/b/.bashrc"), "");
    assert_eq!(node_extname("/a/b/name.tar.gz"), ".gz");
    assert_eq!(node_extname("/a/b/trailing."), ".");
    assert_eq!(node_extname("noext"), "");
    assert_eq!(node_extname("C:\\dir\\file.MP4"), ".MP4");
}

#[test]
fn workspace_creation_methods_match_ts_paths_and_validation() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();

    // 默认工作区：<home>/ESCodeProject，幂等（重复调用成功）。
    let Wire::Object(created) = create_default_workspace(home).expect("create") else {
        panic!("object")
    };
    let expected = home.join("ESCodeProject");
    assert_eq!(
        created["path"].as_str(),
        Some(expected.to_string_lossy().as_ref())
    );
    assert!(expected.is_dir());
    create_default_workspace(home).expect("幂等");

    // 命名工作区：<home>/ESCodeProject/<name>；名字做 trim。
    let Wire::Object(created) =
        create_scratch_workspace(home, &json!({"name": "  my-case  "})).expect("create")
    else {
        panic!("object")
    };
    let named = expected.join("my-case");
    assert_eq!(
        created["path"].as_str(),
        Some(named.to_string_lossy().as_ref())
    );
    assert!(named.is_dir());

    // 名称校验：空名与路径分隔符（文案逐字对齐 TS）。
    let error = create_scratch_workspace(home, &json!({"name": "   "})).expect_err("empty");
    assert_eq!(error.1, "Workspace name is required.");
    let error = create_scratch_workspace(home, &json!({"name": "a/b"})).expect_err("slash");
    assert_eq!(error.1, "Workspace name cannot contain path separators.");
    let error = create_scratch_workspace(home, &json!({"name": "a\\b"})).expect_err("backslash");
    assert_eq!(error.1, "Workspace name cannot contain path separators.");

    // 目标被同名文件占位：mkdir recursive 失败（Node EEXIST 文案）。
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, "x").unwrap();
    let error = create_scratch_workspace(dir.path(), &json!({"name": "../blocked/child"})).err();
    // 上面的名字含分隔符会先被校验拦住；换一个不含分隔符但父路径是文件的场景：
    assert!(error.is_some(), "含分隔符的名字必须被拒");
    let file_home = dir.path().join("file-home");
    std::fs::create_dir_all(&file_home).unwrap();
    std::fs::write(file_home.join("ESCodeProject"), "x").unwrap();
    let error = create_default_workspace(&file_home).expect_err("EEXIST");
    assert!(error.1.starts_with("EEXIST:"), "{error:?}");
}

#[test]
fn conversation_workspace_follows_data_base_dir_and_reports_created() {
    let dir = tempfile::tempdir().unwrap();
    let data_base_dir = dir.path().join("data");

    // `<dataBaseDir>/.escode/workspace/default`；首次 created=true（Node `mkdir` 返回创建路径）。
    let Wire::Object(first) =
        ensure_conversation_workspace(&data_base_dir).expect("create conversation workspace")
    else {
        panic!("object")
    };
    let expected = data_base_dir.join(".escode").join("workspace").join("default");
    assert_eq!(
        first["path"].as_str(),
        Some(expected.to_string_lossy().as_ref())
    );
    assert_eq!(first["created"], true);
    assert_eq!(first["workspacePurpose"], "conversation");
    assert!(expected.is_dir());
    assert!(
        data_base_dir.is_dir() && expected.parent().is_some_and(std::path::Path::is_dir),
        "递归建出中间目录"
    );

    // 已存在：created=false（Node recursive mkdir 返回 undefined）。
    let Wire::Object(second) = ensure_conversation_workspace(&data_base_dir).expect("幂等") else {
        panic!("object")
    };
    assert_eq!(second["created"], false);

    // 叶子被同名文件占位：目录断言失败（文案逐字对齐 TS）。
    let blocked_root = dir.path().join("blocked-data");
    std::fs::create_dir_all(blocked_root.join(".escode").join("workspace")).unwrap();
    std::fs::write(
        blocked_root.join(".escode").join("workspace").join("default"),
        "x",
    )
    .unwrap();
    let error = ensure_conversation_workspace(&blocked_root).expect_err("not a directory");
    assert_eq!(error.0, "InvalidRequest");
    assert!(
        error.1.starts_with("Workspace path is not a directory: "),
        "{error:?}"
    );
}

/// dataBaseDir 缺省（Main 未提交）时回显式缺口，不按 HOME 猜路径。
#[tokio::test]
async fn conversation_workspace_without_data_base_dir_is_an_explicit_gap() {
    let channel = crate::domains_file_channel::file_channel(std::path::PathBuf::from("/tmp"), None);
    let handler = channel
        .handler("ensureConversationWorkspace")
        .expect("method table entry");
    let error = handler(crate::wire::Wire::Undefined)
        .await
        .expect_err("explicit gap");
    assert_eq!(error.0, "InvalidRequest");
    assert!(
        error.1.contains("Main must pass dataBaseDir"),
        "{error:?}"
    );
}
