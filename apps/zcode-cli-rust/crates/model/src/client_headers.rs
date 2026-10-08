//! 模型请求的客户端头与归因头（docs/specs/rust-model-request-headers.md）。修复：Rust 原先只发鉴权与内容类型，
//! 缺少 Node 的来源 / 平台 / 归因头，网关无法按会话类型与 trace 归因。
//! 静态头对齐 TS `bootstrap/src/model-config.ts`，归因头对齐 `adapters/src/model/runner-attribution.ts`。
use crate::contract::ModelCallScope;
use std::sync::OnceLock;

const DEFAULT_ENDPOINT_ORIGIN: &str = "https://zcode.z.ai";
const SESSION_PREFIXES: [&str; 2] = ["sess_", "subagent_agent_"];
const QUERY_PREFIX: &str = "query_";
const OPENCODE_ROOT_DOMAIN: &str = "opencode.ai";
const OPENCODE_GO_PATH: &str = "/zen/go/v1";

/// TS `normalizePrintableHeaderValue`：去首尾空白后必须是非空的可打印 ASCII。
fn printable(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty() && trimmed.bytes().all(|b| (0x20..=0x7e).contains(&b)))
        .then(|| trimmed.to_owned())
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// `ZCODE_BASE_URL` ?? `ZCODE_ENDPOINT_ORIGIN` 的 origin（TS resolveRuntimeZCodeEndpointOrigin）。
fn endpoint_origin(base: Option<String>) -> String {
    base.and_then(|value| {
        let url = reqwest::Url::parse(value.trim()).ok()?;
        matches!(url.scheme(), "http" | "https").then(|| url.origin().ascii_serialization())
    })
    .unwrap_or_else(|| DEFAULT_ENDPOINT_ORIGIN.to_owned())
}

/// 进程内不变的客户端头（TS buildCliZCodeSourceHeaders + createRuntimePlatformHeaders）。
pub(crate) fn static_headers() -> &'static [(String, String)] {
    static HEADERS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    HEADERS.get_or_init(|| {
        let platform = zcode_cli_host::client_platform::current();
        let electron = std::env::args().any(|a| a == "app-server" || a == "agent-server");
        build_static(
            &StaticInputs {
                base_url: env("ZCODE_BASE_URL").or_else(|| env("ZCODE_ENDPOINT_ORIGIN")),
                app_version: printable(env("ZCODE_APP_VERSION").as_deref())
                    .or_else(|| Some(env!("CARGO_PKG_VERSION").to_owned())),
                electron,
                release_test: env("ZCODE_ENV").is_some_and(|v| v.trim().eq_ignore_ascii_case("test")),
            },
            platform,
        )
    })
}

struct StaticInputs {
    base_url: Option<String>,
    app_version: Option<String>,
    electron: bool,
    release_test: bool,
}

fn build_static(
    input: &StaticInputs,
    platform: &zcode_cli_host::client_platform::ClientPlatform,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("http-referer".to_owned(), endpoint_origin(input.base_url.clone())),
        (
            "user-agent".to_owned(),
            format!("ZCode/{}", input.app_version.as_deref().unwrap_or("unknown")),
        ),
    ];
    if let Some(version) = &input.app_version {
        headers.push(("x-zcode-app-version".into(), version.clone()));
    }
    let title = if input.electron { "electron" } else { "cli" };
    headers.push(("x-title".into(), format!("Z Code@{title}")));
    headers.push((
        "x-release-channel".into(),
        if input.release_test { "test" } else { "production" }.into(),
    ));
    headers.push((
        "x-client-language".into(),
        printable(platform.locale.as_deref()).unwrap_or_else(|| "unknown".into()),
    ));
    headers.push((
        "x-client-timezone".into(),
        printable(platform.timezone.as_deref()).unwrap_or_else(|| "unknown".into()),
    ));
    headers.push(("x-zcode-agent".into(), "glm".into()));
    if let (Some(p), Some(a)) = (printable(Some(platform.platform)), printable(Some(platform.arch))) {
        headers.push(("x-platform".into(), format!("{p}-{a}")));
    }
    headers.push(("x-os-category".into(), platform.os_category.into()));
    if let Some(release) = printable(platform.os_release.as_deref()) {
        headers.push(("x-os-version".into(), release));
    }
    headers
}

/// TS stripHeaderInternalPrefixes：按序剥离内部前缀；剥空时保留原值。
fn strip_prefixes(value: &str, prefixes: &[&str]) -> String {
    let mut out = value;
    for prefix in prefixes {
        if out.len() > prefix.len()
            && let Some(rest) = out.strip_prefix(prefix)
        {
            out = rest;
        }
    }
    if out.is_empty() { value } else { out }.to_owned()
}

/// TS resolveModelRequestSessionType：只有主代理与子代理的 agent step 区分，其余为 other。
fn session_type(query_source: Option<&str>) -> &'static str {
    match query_source {
        Some("main_turn") => "main",
        Some("subagent") => "subagent",
        _ => "other",
    }
}

/// TS isOpenCodeGoBaseUrl。
fn is_opencode_go(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = url.path().trim_end_matches('/').to_ascii_lowercase();
    (host == OPENCODE_ROOT_DOMAIN || host.ends_with(&format!(".{OPENCODE_ROOT_DOMAIN}")))
        && path == OPENCODE_GO_PATH
}

/// 每次 HTTP 尝试的归因头（TS createModelRequestAttributionHeaders）；`x-request-id` 每次新生成。
pub(crate) fn attribution(scope: &ModelCallScope, url: &str) -> Vec<(String, String)> {
    let mut headers = vec![
        ("x-request-id".to_owned(), uuid::Uuid::new_v4().to_string()),
        (
            "x-zcode-session-type".to_owned(),
            session_type(scope.query_source.as_deref()).to_owned(),
        ),
        (
            "x-zcode-trace-id".to_owned(),
            scope.trace_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        ),
    ];
    if let Some(turn) = scope.turn_id.as_deref().filter(|t| !t.is_empty()) {
        headers.push(("x-query-id".into(), strip_prefixes(turn, &[QUERY_PREFIX])));
    }
    if let Some(session) = scope.session_id.as_deref().filter(|s| !s.is_empty()) {
        let value = strip_prefixes(session, &SESSION_PREFIXES);
        if is_opencode_go(url) {
            headers.push(("x-opencode-session".into(), value.clone()));
        }
        headers.push(("x-session-id".into(), value));
    }
    headers
}

/// 一次 HTTP 尝试的非鉴权请求头：客户端头 → Provider 配置头 → 归因头（同名覆盖）。`beta_override` 时配置里的
/// `anthropic-beta` 由调用方合并后另行设置。
pub(crate) fn request_headers(
    configured: &std::collections::BTreeMap<String, String>,
    beta_override: bool,
    url: &str,
) -> Vec<(String, String)> {
    let configured: Vec<(String, String)> = configured
        .iter()
        .filter(|(key, _)| !beta_override || !key.eq_ignore_ascii_case("anthropic-beta"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let attribution = attribution(&crate::contract::current_model_call(), url);
    merge(&[static_headers(), &configured, &attribution])
}

/// 合并：后出现的同名（不区分大小写）覆盖先出现的，保持首次出现的位置。
pub(crate) fn merge(layers: &[&[(String, String)]]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for layer in layers {
        for (key, value) in layer.iter() {
            match out.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
                Some(slot) => slot.1 = value.clone(),
                None => out.push((key.clone(), value.clone())),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcode_cli_host::client_platform::ClientPlatform;

    fn platform() -> ClientPlatform {
        ClientPlatform {
            platform: "win32",
            arch: "x64",
            os_category: "windows",
            os_release: Some("10.0.26100".into()),
            locale: Some("zh-CN".into()),
            timezone: Some("Asia/Shanghai".into()),
        }
    }

    #[test]
    fn static_headers_follow_ts_model_config() {
        let headers = build_static(
            &StaticInputs {
                base_url: Some("https://example.test:8443/path".into()),
                app_version: Some("3.14.0".into()),
                electron: true,
                release_test: false,
            },
            &platform(),
        );
        let get = |k: &str| headers.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("http-referer"), Some("https://example.test:8443"));
        assert_eq!(get("user-agent"), Some("ZCode/3.14.0"));
        assert_eq!(get("x-zcode-app-version"), Some("3.14.0"));
        assert_eq!(get("x-title"), Some("Z Code@electron"));
        assert_eq!(get("x-release-channel"), Some("production"));
        assert_eq!(get("x-client-language"), Some("zh-CN"));
        assert_eq!(get("x-client-timezone"), Some("Asia/Shanghai"));
        assert_eq!(get("x-zcode-agent"), Some("glm"));
        assert_eq!(get("x-platform"), Some("win32-x64"));
        assert_eq!(get("x-os-category"), Some("windows"));
        assert_eq!(get("x-os-version"), Some("10.0.26100"));
        let fallback = build_static(
            &StaticInputs { base_url: Some("not a url".into()), app_version: None, electron: false, release_test: true },
            &ClientPlatform { locale: Some("日本".into()), timezone: None, ..platform() },
        );
        let get = |k: &str| fallback.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("http-referer"), Some(DEFAULT_ENDPOINT_ORIGIN));
        assert_eq!(get("user-agent"), Some("ZCode/unknown"));
        assert_eq!(get("x-zcode-app-version"), None);
        assert_eq!(get("x-title"), Some("Z Code@cli"));
        assert_eq!(get("x-release-channel"), Some("test"));
        assert_eq!(get("x-client-language"), Some("unknown"));
        assert_eq!(get("x-client-timezone"), Some("unknown"));
    }

    #[test]
    fn attribution_strips_internal_prefixes_and_maps_session_type() {
        let scope = ModelCallScope {
            session_id: Some("sess_abc".into()),
            turn_id: Some("query_q1".into()),
            query_source: Some("main_turn".into()),
            trace_id: Some("trace-1".into()),
        };
        let headers = attribution(&scope, "https://api.example.test/v1");
        let get = |k: &str| headers.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("x-zcode-session-type"), Some("main"));
        assert_eq!(get("x-zcode-trace-id"), Some("trace-1"));
        assert_eq!(get("x-session-id"), Some("abc"));
        assert_eq!(get("x-query-id"), Some("q1"));
        assert_eq!(get("x-opencode-session"), None);
        assert_ne!(
            get("x-request-id"),
            attribution(&scope, "").iter().find(|(k, _)| k == "x-request-id").map(|(_, v)| v.as_str())
        );
        assert_eq!(strip_prefixes("sess_subagent_agent_x", &SESSION_PREFIXES), "x");
        assert_eq!(strip_prefixes("sess_", &SESSION_PREFIXES), "sess_");
        assert_eq!(session_type(Some("subagent")), "subagent");
        assert_eq!(session_type(Some("session_title")), "other");
        assert_eq!(session_type(None), "other");
        let open = attribution(&scope, "https://x.opencode.ai/zen/go/v1/");
        assert!(open.iter().any(|(k, v)| k == "x-opencode-session" && v == "abc"));
        assert!(!is_opencode_go("https://opencode.ai/zen/v1"));
    }

    #[test]
    fn merge_overrides_case_insensitively_without_duplicates() {
        let a = vec![("x-zcode-session-type".to_owned(), "other".to_owned()), ("User-Agent".to_owned(), "a".to_owned())];
        let b = vec![("X-ZCode-Session-Type".to_owned(), "main".to_owned())];
        let merged = merge(&[&a, &b]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].1, "main");
    }
}
