#[cfg(test)]
use super::*;
use crate::domains_file_preview::node_extname;

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
