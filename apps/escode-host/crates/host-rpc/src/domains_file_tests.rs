#[cfg(test)]
use super::*;

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
