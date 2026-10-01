use super::*;

/// 按 URL 返回预设响应；记录请求顺序。
struct Fake {
    routes: Vec<(
        &'static str,
        u16,
        Vec<(&'static str, &'static str)>,
        &'static str,
    )>,
    seen: Mutex<Vec<String>>,
}

#[async_trait]
impl Transport for Fake {
    async fn get(&self, url: &Url, _: &CancellationToken) -> Result<Response> {
        self.seen.lock().unwrap().push(url.to_string());
        let (_, status, headers, body) = self
            .routes
            .iter()
            .find(|(u, ..)| *u == url.as_str())
            .ok_or_else(|| anyhow!("no route {url}"))?;
        Ok(Response {
            status: *status,
            status_text: String::new(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
        })
    }
}

fn fake(
    routes: Vec<(
        &'static str,
        u16,
        Vec<(&'static str, &'static str)>,
        &'static str,
    )>,
) -> Fake {
    Fake {
        routes,
        seen: Mutex::new(vec![]),
    }
}

async fn run(transport: &Fake, url: &str) -> Result<WebFetchPage> {
    fetch(
        transport,
        &json!({"url": url, "prompt": "p"}),
        None,
        &CancellationToken::new(),
    )
    .await
}

// 缓存是进程级的，所有用例串行放在一个测试里，避免互相污染。
#[tokio::test]
async fn fetch_pipeline_matches_ts_rules() {
    clear_cache();
    let html = vec![("content-type", "text/html; charset=utf-8")];
    // http 升级为 https，同主机（忽略 www）重定向被跟随，正文转 Markdown。
    let t = fake(vec![
        (
            "https://example.com/a",
            301,
            vec![("location", "https://www.example.com/b")],
            "",
        ),
        (
            "https://www.example.com/b",
            200,
            html.clone(),
            "<h1>Hi</h1><p>x</p>",
        ),
    ]);
    let page = run(&t, "http://example.com/a").await.unwrap();
    assert_eq!(page.content.as_deref(), Some("# Hi\nx"));
    assert_eq!(page.output["finalUrl"], "https://www.example.com/b");
    assert_eq!(page.output["redirects"][0]["status"], 301);
    assert_eq!(page.output["cacheHit"], false);
    assert_eq!(page.output["statusText"], "OK");
    // 同一原始 url 第二次命中缓存，不再请求。
    let again = run(&t, "http://example.com/a").await.unwrap();
    assert_eq!(again.output["cacheHit"], true);
    assert_eq!(t.seen.lock().unwrap().len(), 2);

    // 跨主机重定向返回终态文案，不跟随。
    let t = fake(vec![(
        "https://example.org/",
        302,
        vec![("location", "https://other.org/x")],
        "",
    )]);
    let page = run(&t, "https://example.org/").await.unwrap();
    assert!(page.content.is_none());
    let result = page.output["result"].as_str().unwrap();
    assert!(result.starts_with("REDIRECT DETECTED"));
    assert!(result.contains("Redirect URL: https://other.org/x\nStatus: 302 Found"));
    assert!(result.ends_with("- prompt: \"p\""));
    assert_eq!(page.output["bytes"], result.len());

    // HTTP 错误：Retry-After 仅接受 1~6 位数字。
    let t = fake(vec![(
        "https://example.net/",
        429,
        vec![("retry-after", "30")],
        "busy",
    )]);
    let page = run(&t, "https://example.net/").await.unwrap();
    assert!(
        page.output["result"]
            .as_str()
            .unwrap()
            .starts_with("The server returned HTTP 429 Too Many Requests.\nRetry-After: 30\n\n")
    );
    assert_eq!(page.output["bytes"], 0);

    // 代理 allowlist 拦截与超过重定向上限都是错误。
    let t = fake(vec![(
        "https://blocked.com/",
        403,
        vec![("x-proxy-error", "blocked-by-allowlist")],
        "",
    )]);
    let error = run(&t, "https://blocked.com/")
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        r#"{"error_type":"EGRESS_BLOCKED","domain":"blocked.com","message":"Access to blocked.com is blocked by the network egress proxy."}"#
    );
    let t = fake(vec![(
        "https://loop.com/",
        307,
        vec![("location", "/")],
        "",
    )]);
    let error = run(&t, "https://loop.com/").await.unwrap_err().to_string();
    assert_eq!(error, "WebFetch exceeded the safe redirect limit");
    assert_eq!(t.seen.lock().unwrap().len(), 11);

    // 字面量私网地址在请求前被拦截；本地主机名在 URL 层被拒绝。
    let t = fake(vec![]);
    let error = run(&t, "https://10.0.0.1/").await.unwrap_err().to_string();
    assert_eq!(
        error,
        "WebFetch cannot access private or local IP addresses"
    );
    let error = run(&t, "https://localhost/").await.unwrap_err().to_string();
    assert_eq!(error, "WebFetch requires a public hostname");
    assert!(t.seen.lock().unwrap().is_empty());
}

/// TS `maybePersistRawContent`：抽取后的正文（UTF-8 字节）超过 100k 时写 tool-result artifact，
/// 文件名与 URI 同 `writeToolResultArtifact`；命中缓存复用同一 artifact，不重复落盘。
#[tokio::test]
async fn oversized_content_writes_a_tool_result_artifact() {
    clear_cache();
    let body: &'static str = Box::leak(format!("<p>{}</p>", "x".repeat(120_000)).into_boxed_str());
    let t = fake(vec![(
        "https://big.example.com/",
        200,
        vec![("content-type", "text/html; charset=utf-8")],
        body,
    )]);
    let root =
        std::env::temp_dir().join(format!("zcode-webfetch-artifact-{}", zcode_cli_host::id()));
    let target = ArtifactTarget {
        root: &root,
        session: "sess_test",
        call_id: "call-1",
    };
    let args = json!({"url": "https://big.example.com/", "prompt": "p"});
    let page = fetch(&t, &args, Some(target), &CancellationToken::new())
        .await
        .unwrap();
    let uri = page.output["artifactUri"].as_str().unwrap().to_owned();
    assert!(
        uri.starts_with("zcode-artifact://sess_test/tool-result-"),
        "{uri}"
    );
    let path = page.output["artifactPath"].as_str().unwrap().to_owned();
    let name = std::path::Path::new(&path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with("call-1-tool-result-"), "{name}");
    assert!(name.ends_with(".md"), "{name}");
    // 落盘内容就是抽取后的正文（HTML 转 Markdown）。
    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        page.content.clone().unwrap()
    );
    // 同一 URL 第二次命中缓存：artifact 字段复用，不再请求也不新增文件。
    let again = fetch(&t, &args, Some(target), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(again.output["cacheHit"], true);
    assert_eq!(again.output["artifactUri"], Value::from(uri));
    assert_eq!(again.output["artifactPath"], Value::from(path));
    assert_eq!(t.seen.lock().unwrap().len(), 1);
    let _ = tokio::fs::remove_dir_all(&root).await;
}
