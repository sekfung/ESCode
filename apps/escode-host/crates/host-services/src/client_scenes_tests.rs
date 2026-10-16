use super::*;

fn origin(pairs: &[(&str, &str)]) -> String {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    resolve_runtime_origin(|name| map.get(name).map(String::as_str))
}

#[test]
fn runtime_origin_follows_the_ts_precedence() {
    // 向量由真实 TS `resolveRuntimeESCodeEndpointOrigin` 运行得到。// 缺省 = 生产默认；`test`（任意大小写）才切到测试作用域变量。
    assert_eq!(origin(&[]), "https://zcode.z.ai");
    assert_eq!(
        origin(&[("ESCODE_PRODUCTION_BASE_URL", "https://prod.example.com/x")]),
        "https://prod.example.com"
    );
    assert_eq!(
        origin(&[("ESCODE_TEST_BASE_URL", "https://test.example.com")]),
        "https://zcode.z.ai",
        "生产环境不读测试作用域变量"
    );
    assert_eq!(
        origin(&[
            ("ESCODE_ENV", "TEST"),
            ("ESCODE_TEST_BASE_URL", "https://test.example.com")
        ]),
        "https://test.example.com"
    );
    // ESCODE_BASE_URL > ESCODE_ENDPOINT_ORIGIN > 作用域变量；空白视为缺省。
    assert_eq!(
        origin(&[
            ("ESCODE_BASE_URL", "https://a.example.com"),
            ("ESCODE_ENDPOINT_ORIGIN", "https://b.example.com"),
            ("ESCODE_PRODUCTION_BASE_URL", "https://c.example.com"),
        ]),
        "https://a.example.com"
    );
    assert_eq!(
        origin(&[
            ("ESCODE_BASE_URL", "  "),
            ("ESCODE_ENDPOINT_ORIGIN", "https://b.example.com")
        ]),
        "https://b.example.com"
    );
    // 非 production/test 的 ESCODE_ENV 一律按生产（TS：只有 `test` 才是测试环境）。
    assert_eq!(origin(&[("ESCODE_ENV", "test")]), "https://zcode.z.ai");
    assert_eq!(
        origin(&[
            ("ESCODE_ENV", "staging"),
            ("ESCODE_PRODUCTION_BASE_URL", "https://p.example.com")
        ]),
        "https://p.example.com"
    );
    // 差异（已登记）：非法 Origin 在 TS 里让模块加载抛错，Host 退回默认 Origin。
    assert_eq!(
        origin(&[("ESCODE_BASE_URL", "ftp://x.example.com")]),
        "https://zcode.z.ai"
    );
}
