use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::json;

use super::super::service_tests::builtin;
use super::*;

/// 按 URL 路径回放的桩；记录请求 URL（断言 query 与顺序）。
struct Stub {
    replies: HashMap<String, Result<Value, String>>,
    seen: Mutex<Vec<String>>,
    delay: Option<Duration>,
}

impl Stub {
    fn new(replies: &[(&str, Result<Value, String>)]) -> Self {
        Self {
            replies: replies
                .iter()
                .map(|(path, reply)| ((*path).to_owned(), reply.clone()))
                .collect(),
            seen: Mutex::default(),
            delay: None,
        }
    }
}

#[async_trait]
impl JsonGet for Stub {
    async fn get_json(&self, url: &Url) -> Result<Value, String> {
        self.seen.lock().unwrap().push(url.to_string());
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        self.replies
            .get(url.path())
            .cloned()
            .unwrap_or_else(|| Err("HTTP 404".to_owned()))
    }
}

fn configs(url: Option<&str>) -> Value {
    let mut configs = json!({"other": 1});
    if let Some(url) = url {
        configs["builtin_provider_config_json"] = json!(url);
    }
    json!({"code": 0, "data": {"configs": configs}})
}

async fn run(stub: &Stub) -> Result<Option<Value>, String> {
    download_release(
        "https://api.example.com",
        "1.2.3",
        "linux-x86_64",
        stub,
        TOTAL_BUDGET,
    )
    .await
}

#[tokio::test]
async fn downloads_the_release_named_by_client_config() {
    let stub = Stub::new(&[
        (
            "/api/v1/client/configs",
            Ok(configs(Some("https://cdn.example.com/rel.json?sig=secret"))),
        ),
        ("/rel.json", Ok(builtin())),
    ]);
    assert_eq!(run(&stub).await.unwrap(), Some(builtin()));
    let seen = stub.seen.lock().unwrap().clone();
    assert_eq!(
        seen[0],
        "https://api.example.com/api/v1/client/configs?app_version=1.2.3&platform=linux-x86_64"
    );
    assert_eq!(seen.len(), 2);
}

#[tokio::test]
async fn a_missing_field_means_no_release_not_an_error() {
    let stub = Stub::new(&[("/api/v1/client/configs", Ok(configs(None)))]);
    assert_eq!(run(&stub).await.unwrap(), None);
}

#[tokio::test]
async fn rejects_unsafe_or_malformed_client_config() {
    for payload in [
        configs(Some("http://cdn.example.com/rel.json")),
        configs(Some("https://user:pw@cdn.example.com/rel.json")),
        configs(Some("https://user@cdn.example.com/rel.json")),
        configs(Some("not a url")),
        json!({"code": 1, "data": {"configs": {}}}),
        json!({"code": 0}),
        json!({"code": 0, "data": {"configs": {"builtin_provider_config_json": null}}}),
    ] {
        let stub = Stub::new(&[("/api/v1/client/configs", Ok(payload.clone()))]);
        assert_eq!(
            run(&stub).await.unwrap_err(),
            "ESCode Built-in client-config: invalid response",
            "{payload}"
        );
    }
}

#[tokio::test]
async fn errors_name_the_stage_and_never_leak_urls() {
    let stub = Stub::new(&[("/api/v1/client/configs", Err("HTTP 500".into()))]);
    assert_eq!(
        run(&stub).await.unwrap_err(),
        "ESCode Built-in client-config: HTTP 500"
    );
    let stub = Stub::new(&[
        (
            "/api/v1/client/configs",
            Ok(configs(Some("https://cdn.example.com/rel.json?sig=secret"))),
        ),
        ("/rel.json", Err("body limit exceeded".into())),
    ]);
    let error = run(&stub).await.unwrap_err();
    assert_eq!(error, "ESCode Built-in cdn: body limit exceeded");
    assert!(!error.contains("secret") && !error.contains("cdn.example.com"));
}

#[tokio::test]
async fn an_invalid_or_retired_release_is_rejected() {
    let mut retired = builtin();
    retired["config"]["providerConfigRules"]["providerRules"][0]["providerId"] =
        json!("builtin:zapi");
    for release in [json!({"schemaVersion": 1}), retired] {
        let stub = Stub::new(&[
            (
                "/api/v1/client/configs",
                Ok(configs(Some("https://cdn.example.com/rel.json"))),
            ),
            ("/rel.json", Ok(release)),
        ]);
        assert_eq!(
            run(&stub).await.unwrap_err(),
            "ESCode Built-in cdn: invalid response"
        );
    }
}

// 虚拟时间：Windows 的墙钟定时器按系统刻度唤醒，50 ms 的 timeout 与第二次 30 ms sleep
// （合计 60 ms）可能在同一刻度就绪；`tokio::time::Timeout` 先轮询内部 future，请求完成即返回，
// 超时被跳过，于是本应失败的用例间歇性通过。暂停时钟后运行时只推进到下一个定时器截止点
// （first sleep 30 ms → timeout 50 ms），结果确定。
#[tokio::test(start_paused = true)]
async fn the_budget_covers_both_requests_together() {
    let mut stub = Stub::new(&[
        (
            "/api/v1/client/configs",
            Ok(configs(Some("https://cdn.example.com/rel.json"))),
        ),
        ("/rel.json", Ok(builtin())),
    ]);
    // 每个请求 30 ms：单个都在 50 ms 预算内，合计 60 ms 超出，且发生在 cdn 阶段。
    stub.delay = Some(Duration::from_millis(30));
    let error = download_release(
        "https://api.example.com",
        "1.2.3",
        "linux-x86_64",
        &stub,
        Duration::from_millis(50),
    )
    .await
    .unwrap_err();
    assert_eq!(error, "ESCode Built-in cdn: timeout");
}
