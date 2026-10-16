use super::*;

fn request_of(message: ControlMessage) -> (String, String) {
    match message {
        ControlMessage::LocalMediaPreviewAuthorize { request_id, path } => (request_id, path),
        other => panic!("unexpected message {other:?}"),
    }
}

#[tokio::test]
async fn replies_are_paired_by_request_id_even_out_of_order() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let authorizer = PathAuthorizer::new(writer);
    let (a, b) = (Arc::clone(&authorizer), Arc::clone(&authorizer));
    let first = tokio::spawn(async move { a.authorize("/a.mp4").await });
    let (id_a, path_a) = request_of(rx.recv().await.unwrap());
    let second = tokio::spawn(async move { b.authorize("/b.mp4").await });
    let (id_b, path_b) = request_of(rx.recv().await.unwrap());
    assert_eq!((path_a.as_str(), path_b.as_str()), ("/a.mp4", "/b.mp4"));
    assert_ne!(id_a, id_b);
    // 先答第二个：配对不能依赖到达顺序。
    assert!(authorizer.resolve(&id_b, true, Some("/real/b.mp4".into()), None));
    assert!(authorizer.resolve(&id_a, true, Some("/real/a.mp4".into()), None));
    assert_eq!(first.await.unwrap().unwrap(), "/real/a.mp4");
    assert_eq!(second.await.unwrap().unwrap(), "/real/b.mp4");
}

#[tokio::test]
async fn unknown_and_repeated_replies_are_dropped() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let authorizer = PathAuthorizer::new(writer);
    assert!(!authorizer.resolve("media-999", true, Some("/x".into()), None));
    let waiting = {
        let authorizer = Arc::clone(&authorizer);
        tokio::spawn(async move { authorizer.authorize("/a.mp3").await })
    };
    let (id, _) = request_of(rx.recv().await.unwrap());
    assert!(authorizer.resolve(&id, true, Some("/a.mp3".into()), None));
    assert!(
        !authorizer.resolve(&id, true, Some("/other".into()), None),
        "只消费一次"
    );
    assert_eq!(waiting.await.unwrap().unwrap(), "/a.mp3");
}

#[tokio::test]
async fn main_errors_are_forwarded_verbatim() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let authorizer = PathAuthorizer::new(writer);
    let waiting = {
        let authorizer = Arc::clone(&authorizer);
        tokio::spawn(async move { authorizer.authorize("/a.mp3").await })
    };
    let (id, _) = request_of(rx.recv().await.unwrap());
    authorizer.resolve(&id, false, None, Some("denied by main".into()));
    assert_eq!(waiting.await.unwrap().unwrap_err(), "denied by main");
}

#[tokio::test]
async fn ok_without_a_path_is_an_error() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let authorizer = PathAuthorizer::new(writer);
    let waiting = {
        let authorizer = Arc::clone(&authorizer);
        tokio::spawn(async move { authorizer.authorize("/a.mp3").await })
    };
    let (id, _) = request_of(rx.recv().await.unwrap());
    authorizer.resolve(&id, true, None, None);
    assert!(
        waiting
            .await
            .unwrap()
            .unwrap_err()
            .contains("returned no path")
    );
}

#[tokio::test]
async fn close_fails_pending_requests_and_releases_the_queue() {
    let (writer, mut rx) = ControlWriter::channel(8);
    let authorizer = PathAuthorizer::new(writer);
    let waiting = {
        let authorizer = Arc::clone(&authorizer);
        tokio::spawn(async move { authorizer.authorize("/a.mp3").await })
    };
    let _ = rx.recv().await.unwrap();
    authorizer.close();
    assert_eq!(
        waiting.await.unwrap().unwrap_err(),
        "Control channel closed"
    );
    assert_eq!(
        authorizer.authorize("/b.mp3").await.unwrap_err(),
        "Control channel closed"
    );
    // 写句柄已释放：队列在没有其它发送者时关闭（泵得以退出）。
    assert!(rx.recv().await.is_none());
}
