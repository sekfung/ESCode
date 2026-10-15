use super::*;

#[test]
fn media_format_is_case_insensitive_and_backslash_tolerant() {
    assert_eq!(media_format("/a/b.MP3"), Some(("audio/mpeg", "audio")));
    assert_eq!(
        media_format("C:\\clips\\x.Mov"),
        Some(("video/quicktime", "video"))
    );
    assert_eq!(media_format("/a/b.txt"), None);
    assert_eq!(media_format("/a/b.mp3.bak"), None);
}

#[test]
fn prepare_rejects_a_kind_mismatch_and_directories() {
    let dir = tempfile::tempdir().expect("dir");
    let song = dir.path().join("song.mp3");
    std::fs::write(&song, b"abc").expect("write");
    let song_path = song.to_string_lossy().into_owned();
    let mismatch = prepare(&json!({"path": song_path, "expectedKind": "video"})).expect_err("kind");
    assert!(
        mismatch
            .message
            .starts_with("Unsupported media preview format")
    );
    let folder = dir.path().join("dir.mp4");
    std::fs::create_dir(&folder).expect("mkdir");
    let not_file = prepare(&json!({"path": folder.to_string_lossy(), "expectedKind": "video"}))
        .expect_err("dir");
    assert!(not_file.message.starts_with("Path is not a media file"));
    let ok = prepare(&json!({"path": song_path, "expectedKind": "audio"})).expect("ok");
    let Wire::Object(value) = ok else {
        panic!("object")
    };
    assert_eq!(value["kind"], "inline");
    assert_eq!(value["size"], 3);
    assert_eq!(value["dataBase64"], "YWJj");
}
