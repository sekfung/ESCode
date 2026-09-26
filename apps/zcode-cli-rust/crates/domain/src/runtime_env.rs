//! 子进程环境规则（docs/specs/rust-browser-use.md「第 4 期细则」），逐项对齐 TS
//! `packages/shared/src/runtimeEnv.ts` 的清洗清单与 `adapters/src/network/subprocess-env.ts` 的出网恢复。
//! 纯函数：输入启动环境，输出对子进程的差量（`None` 表示删除）。
use serde_json::Value;

pub const TOOL_ENV_PASSTHROUGH_KEY: &str = "ZCODE_TOOL_ENV_PASSTHROUGH_JSON";
pub const CUA_SOCKET_KEY: &str = "ZCODE_CUA_PERMISSION_BROKER_SOCKET";
pub const CUA_AUTHORITY_KEY: &str = "ZCODE_CUA_PLUGIN_AUTHORITY";
pub const CUA_REFRESH_MARKER_KEY: &str = "ZCODE_CUA_PERMISSION_BROKER_REFRESH_MARKER";
const CUA_TOKEN_KEY: &str = "ZCODE_CUA_PERMISSION_BROKER_TOKEN";

const SANITIZED: &[&str] = &[
    "NODE_ENV",
    "ELECTRON_RUN_AS_NODE",
    "NODE_NO_WARNINGS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "ZCODE_REMOTE_RUNTIME_NETWORK_AUTHORITY",
    "ZCODE_REMOTE_HTTP_PROXY",
    "ZCODE_REMOTE_NO_PROXY",
    CUA_SOCKET_KEY,
    CUA_TOKEN_KEY,
    CUA_REFRESH_MARKER_KEY,
    CUA_AUTHORITY_KEY,
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_HEADERS",
    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
    "OTEL_SERVICE_NAME",
    "OTEL_RESOURCE_ATTRIBUTES",
    "OTEL_EXPORTER_OTLP_COMPRESSION",
    "ZCODE_MODEL_TELEMETRY_ENABLED",
    "ZCODE_TELEMETRY_DEVICE_MID",
    "ZCODE_TELEMETRY_USER_ID",
    "ZCODE_TELEMETRY_USER_ID_HASH",
    "ZCODE_TELEMETRY_USER_SUBJECT_ID",
    "ZCODE_TELEMETRY_IDENTITY_STATE",
    "ZCODE_TELEMETRY_RUNTIME_SURFACE",
    "ZCODE_TELEMETRY_RUNTIME_DISTRIBUTION",
];

const NON_PASSTHROUGH: &[&str] = &[
    "NODE_ENV",
    "ELECTRON_RUN_AS_NODE",
    "NODE_NO_WARNINGS",
    CUA_SOCKET_KEY,
    CUA_REFRESH_MARKER_KEY,
    CUA_AUTHORITY_KEY,
    "ZCODE_REMOTE_RUNTIME_NETWORK_AUTHORITY",
    "ZCODE_REMOTE_HTTP_PROXY",
    "ZCODE_REMOTE_NO_PROXY",
    // TS 注释要求剔除遗留 bearer token，但透传捕获会把它恢复给工具子进程；Rust 不恢复（有意差异）。
    CUA_TOKEN_KEY,
];

const PROXY_KEYS: [&str; 6] = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"];
const NO_PROXY_KEYS: [&str; 2] = ["NO_PROXY", "no_proxy"];
const CA_KEYS: [&str; 5] = [
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
];

fn is_telemetry(upper: &str) -> bool {
    upper.starts_with("OTEL_") || upper.starts_with("ZCODE_TELEMETRY_") || upper == "ZCODE_MODEL_TELEMETRY_ENABLED"
}

/// TS `shouldSanitizeZCodeRuntimeEnvKey`：清单按大写比较，包管理器模式不区分大小写。
pub fn is_sanitized(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if SANITIZED.contains(&upper.as_str()) {
        return true;
    }
    let lower = key.to_ascii_lowercase();
    ["npm_config_", "yarn_", "pnpm_"].iter().any(|prefix| {
        lower.strip_prefix(prefix).is_some_and(|rest| {
            matches!(rest, "http_proxy" | "https_proxy" | "proxy" | "all_proxy" | "no_proxy" | "cafile" | "ca")
        })
    })
}

/// TS `shouldCaptureZCodeToolEnvPassthroughKey`。
pub fn is_passthrough_capturable(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    !is_telemetry(&upper) && !NON_PASSTHROUGH.contains(&upper.as_str()) && is_sanitized(key)
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// TS `buildZCodeToolEnvPassthroughEnv`：已有 JSON 的合法可捕获键，再由启动环境的可捕获键覆盖。
fn passthrough(env: &[(String, String)]) -> Vec<(String, String)> {
    let mut captured: Vec<(String, String)> = vec![];
    let mut put = |key: &str, value: &str| {
        captured.retain(|(k, _)| k != key);
        captured.push((key.to_owned(), value.to_owned()));
    };
    if let Some((_, raw)) = env.iter().find(|(k, _)| k == TOOL_ENV_PASSTHROUGH_KEY)
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw)
    {
        for (key, value) in &map {
            if let Some(value) = value.as_str()
                && valid_key(key)
                && is_passthrough_capturable(key)
            {
                put(key, value);
            }
        }
    }
    for (key, value) in env {
        if is_passthrough_capturable(key) {
            put(key, value);
        }
    }
    captured
}

/// 子进程差量。`tool` 为 Bash/自定义命令/MCP stdio；其余内部子进程只清洗。
/// `windows` 时环境键大小写不敏感：写入前删除所有同名变体。
pub fn child_env(env: &[(String, String)], tool: bool, windows: bool) -> Vec<(String, Option<String>)> {
    let mut changes: Vec<(String, Option<String>)> = vec![];
    let same = |a: &str, b: &str| if windows { a.eq_ignore_ascii_case(b) } else { a == b };
    let remove = |changes: &mut Vec<(String, Option<String>)>, key: &str| {
        for (existing, _) in env.iter().filter(|(k, _)| same(k, key)) {
            changes.push((existing.clone(), None));
        }
        changes.push((key.to_owned(), None));
    };
    for (key, _) in env.iter().filter(|(k, _)| is_sanitized(k)) {
        changes.push((key.clone(), None));
    }
    if !tool {
        return changes;
    }
    let lookup = |key: &str| {
        env.iter()
            .find(|(k, _)| same(k, key))
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let set = |changes: &mut Vec<(String, Option<String>)>, key: &str, value: String| {
        remove(changes, key);
        changes.push((key.to_owned(), Some(value)));
    };
    remove(&mut changes, TOOL_ENV_PASSTHROUGH_KEY);
    for (key, value) in passthrough(env) {
        set(&mut changes, &key, value);
    }
    if let Some(proxy) = lookup("ZCODE_HTTP_PROXY") {
        let proxy = if has_scheme(&proxy) { proxy } else { format!("http://{proxy}") };
        for key in PROXY_KEYS {
            set(&mut changes, key, proxy.clone());
        }
    }
    if let Some(no_proxy) = env.iter().find(|(k, _)| same(k, "ZCODE_NO_PROXY")).map(|(_, v)| v.clone()).filter(|v| !v.trim().is_empty()) {
        for key in NO_PROXY_KEYS {
            set(&mut changes, key, no_proxy.clone());
        }
    }
    if let Some(ca) = env.iter().find(|(k, _)| same(k, "ZCODE_AGENT_CA_CERT")).map(|(_, v)| v.clone()).filter(|v| !v.trim().is_empty()) {
        for key in CA_KEYS {
            set(&mut changes, key, ca.clone());
        }
    }
    changes
}

/// TS `normalizeProxyValue` 的协议判定：`^[a-z][a-z0-9+.-]*://`（不区分大小写）。
fn has_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CuaCredentials {
    pub socket: String,
    pub authority: String,
    pub refresh_marker: Option<String>,
}

/// TS `captureZCodeCuaBrokerCredentials`：socket 与 authority 同时非空才成组，半组 fail-closed。
pub fn cua_credentials(env: &[(String, String)]) -> Option<CuaCredentials> {
    let get = |key: &str| {
        env.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    Some(CuaCredentials {
        socket: get(CUA_SOCKET_KEY)?,
        authority: get(CUA_AUTHORITY_KEY)?,
        refresh_marker: get(CUA_REFRESH_MARKER_KEY),
    })
}

#[cfg(test)]
#[path = "runtime_env_tests.rs"]
mod tests;
