use super::*;

fn view() -> Value {
    json!({"providers": [
        {"providerId": "ok", "enabled": true, "executable": true, "models": [
            {"modelId": "m", "enabled": true, "executable": true, "issues": []},
            {"modelId": "off", "enabled": false, "executable": true, "issues": []},
            {"modelId": "bad", "enabled": true, "executable": true, "issues": [{}]},
            {"modelId": "noexec", "enabled": true, "executable": false, "issues": []}]},
        {"providerId": "disabled", "enabled": false, "executable": true, "models": []},
        {"providerId": "noexec", "enabled": true, "executable": false, "models": [
            {"modelId": "m", "enabled": true, "executable": true, "issues": []}]},
    ]})
}

fn code(provider: &str, model: &str) -> Option<String> {
    unavailable(&view(), provider, model)
        .map(|r| r["error"]["code"].as_str().unwrap_or_default().to_owned())
}

#[test]
fn eligibility_chain_matches_the_ts_order() {
    assert_eq!(code("ok", "m"), None);
    assert_eq!(code("ghost", "m").as_deref(), Some("provider-unavailable"));
    assert_eq!(
        code("disabled", "m").as_deref(),
        Some("provider-unavailable")
    );
    assert_eq!(code("ok", "ghost").as_deref(), Some("model-unavailable"));
    assert_eq!(code("ok", "off").as_deref(), Some("model-unavailable"));
    assert_eq!(code("ok", "bad").as_deref(), Some("model-unavailable"));
    assert_eq!(code("noexec", "m").as_deref(), Some("provider-unavailable"));
    assert_eq!(code("ok", "noexec").as_deref(), Some("model-unavailable"));
}
