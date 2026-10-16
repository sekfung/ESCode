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

#[tokio::test]
async fn prepare_rejects_a_kind_mismatch_and_directories() {
    let dir = tempfile::tempdir().expect("dir");
    let song = dir.path().join("song.mp3");
    std::fs::write(&song, b"abc").expect("write");
    let song_path = song.to_string_lossy().into_owned();
    let mismatch = prepare(json!({"path": song_path, "expectedKind": "video"}), None)
        .await
        .expect_err("kind");
    assert!(
        mismatch
            .message
            .starts_with("Unsupported media preview format")
    );
    let folder = dir.path().join("dir.mp4");
    std::fs::create_dir(&folder).expect("mkdir");
    let not_file = prepare(
        json!({"path": folder.to_string_lossy(), "expectedKind": "video"}),
        None,
    )
    .await
    .expect_err("dir");
    assert!(not_file.message.starts_with("Path is not a media file"));
    let ok = prepare(json!({"path": song_path, "expectedKind": "audio"}), None)
        .await
        .expect("ok");
    let Wire::Object(value) = ok else {
        panic!("object")
    };
    assert_eq!(value["kind"], "inline");
    assert_eq!(value["size"], 3);
    assert_eq!(value["dataBase64"], "YWJj");
}

#[test]
fn local_media_url_matches_url_search_params_encoding() {
    // 向量由 TS `buildLocalMediaPreviewUrl` 生成（URLSearchParams：空格 → +，保留 *-._，其余按 UTF-8 百分号编码）。
    for (path, url) in [
        ("/a/b.mp4", "escode-media://local/preview?path=%2Fa%2Fb.mp4"),
        (
            "/a b/c.mp4",
            "escode-media://local/preview?path=%2Fa+b%2Fc.mp4",
        ),
        (
            "/视频/x#1?.mp4",
            "escode-media://local/preview?path=%2F%E8%A7%86%E9%A2%91%2Fx%231%3F.mp4",
        ),
        (
            "C:\\clips\\x~100%.mov",
            "escode-media://local/preview?path=C%3A%5Cclips%5Cx%7E100%25.mov",
        ),
        (
            "/a*b-c_d.e.mp3",
            "escode-media://local/preview?path=%2Fa*b-c_d.e.mp3",
        ),
        (
            "/a'b(1)!.mp4",
            "escode-media://local/preview?path=%2Fa%27b%281%29%21.mp4",
        ),
    ] {
        assert_eq!(local_media_url(path), url, "{path}");
    }
}

/// 模拟 Main：读取控制队列里的授权请求并按 `decide` 回复。
fn simulated_main(
    decide: impl Fn(&str) -> Result<String, String> + Send + 'static,
) -> Arc<PathAuthorizer> {
    use escode_host_control::{ControlMessage, ControlWriter};
    let (writer, mut rx) = ControlWriter::channel(8);
    let authorizer = PathAuthorizer::new(writer);
    let replier = Arc::clone(&authorizer);
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if let ControlMessage::LocalMediaPreviewAuthorize { request_id, path } = message {
                match decide(&path) {
                    Ok(canonical) => replier.resolve(&request_id, true, Some(canonical), None),
                    Err(error) => replier.resolve(&request_id, false, None, Some(error)),
                };
            }
        }
    });
    authorizer
}

#[tokio::test]
async fn an_authorizing_main_turns_prepare_into_a_local_url_without_the_inline_limit() {
    let dir = tempfile::tempdir().expect("dir");
    let clip = dir.path().join("big clip.MP4");
    // 超过内联上限的稀疏文件：local-url 分支不读内容。
    let file = std::fs::File::create(&clip).expect("create");
    file.set_len(MAX_MEDIA_PREVIEW_BYTES + 1).expect("len");
    let clip_path = clip.to_string_lossy().into_owned();
    let authorizer = simulated_main(|path| Ok(format!("{path}.canonical")));
    let value = prepare(
        json!({"path": clip_path, "expectedKind": "video"}),
        Some(authorizer),
    )
    .await
    .expect("local-url");
    let Wire::Object(value) = value else {
        panic!("object")
    };
    assert_eq!(value["kind"], "local-url");
    assert_eq!(value["mediaType"], "video/mp4");
    assert_eq!(value["size"], MAX_MEDIA_PREVIEW_BYTES + 1);
    let canonical = format!("{clip_path}.canonical");
    assert_eq!(value["path"], canonical.as_str());
    assert_eq!(value["url"], local_media_url(&canonical).as_str());
}

#[tokio::test]
async fn main_denying_the_path_fails_prepare_with_its_message() {
    let dir = tempfile::tempdir().expect("dir");
    let song = dir.path().join("song.mp3");
    std::fs::write(&song, b"abc").expect("write");
    let authorizer = simulated_main(|_| Err("not allowed".to_owned()));
    let error = prepare(
        json!({"path": song.to_string_lossy(), "expectedKind": "audio"}),
        Some(authorizer),
    )
    .await
    .expect_err("denied");
    assert_eq!(error.message, "not allowed");
}

#[tokio::test]
async fn validation_runs_before_authorization() {
    let authorizer = simulated_main(|_| panic!("must not ask Main for an invalid path"));
    let error = prepare(
        json!({"path": "/nope/missing.mp4", "expectedKind": "audio"}),
        Some(authorizer),
    )
    .await
    .expect_err("kind");
    assert!(
        error
            .message
            .starts_with("Unsupported media preview format")
    );
}
