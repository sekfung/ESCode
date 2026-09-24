//! 差分：Rust net_proxy 与 TS resolveProxyForRequest / resolveWebFetchProxyForRequest 逐条一致。
//! 语料由 scripts/generate-zcode-cli-rust-proxy-corpus.mjs 生成（url × 显式配置 × 环境变量）。
use serde_json::Value;
use zcode_cli_domain::net_proxy::{
    ProxyOptions, resolve_proxy_for_request, resolve_webfetch_proxy_for_request,
};

fn options(fixture: &Value, oi: usize, ei: usize) -> ProxyOptions {
    let spec = &fixture["options"][oi];
    let env_spec = &fixture["envs"][ei];
    ProxyOptions {
        http_proxy: spec["httpProxy"].as_str().map(str::to_owned),
        no_proxy: spec["noProxy"].as_str().map(str::to_owned),
        env: env_spec
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn same(got: &zcode_cli_domain::net_proxy::ProxyResolution, want: &Value) -> bool {
    got.no_proxy_matched == want["noProxyMatched"].as_bool().unwrap_or(false)
        && got.proxy_source.as_deref() == want["proxySource"].as_str()
        && got.proxy_url.as_deref() == want["proxyUrl"].as_str()
}

#[test]
fn rust_proxy_resolution_matches_ts() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/proxy_corpus.json")).unwrap();
    let mut failures = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let (url, oi, ei) = (
            case[0].as_str().unwrap(),
            case[1].as_u64().unwrap() as usize,
            case[2].as_u64().unwrap() as usize,
        );
        let opts = options(&fixture, oi, ei);
        let got = resolve_proxy_for_request(url, &opts);
        if !same(&got, &case[3]) {
            failures.push(format!("{url} options#{oi} env#{ei}: {got:?} != {}", case[3]));
        }
        let got = resolve_webfetch_proxy_for_request(url, &opts);
        if !same(&got, &case[4]) {
            failures.push(format!("webfetch {url} options#{oi} env#{ei}: {got:?} != {}", case[4]));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures[..failures.len().min(12)].join("\n")
    );
}
