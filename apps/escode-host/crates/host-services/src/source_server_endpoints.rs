//! `kind: "server"` 直连的端点解析（S5-f-1，docs/specs/source-server-remote-rust-host.md §12.6.3）。
//!
//! 与 TS `resolveServerRemoteEndpoints`（`packages/desktop/src/host/serverRemoteConnection.ts`）
//! 逐字节一致：协议互转、剥离尾部 `/ws` / `/ws/host`、拼固定端点、清空 query 与 hash。
//! token 只进入 query 与请求头，不接受调用方把 token 拼进 URL 后传递。

use anyhow::{Context, Result, bail};
use url::Url;

const INFO_PATH: &str = "/api/server-info";
const WS_PATH: &str = "/ws";
const HOST_CAPABILITY_PATH: &str = "/api/rpc-host-capability";
const HOST_WS_PATH: &str = "/ws/host";

/// 一个 server 目标需要的四个端点，全部是绝对 URL。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRemoteEndpoints {
    pub info_url: String,
    pub ws_url: String,
    pub host_capability_url: String,
    pub host_ws_url: String,
}

/// 解析 server URL；`http(s)` 与 `ws(s)` 都接受，互相派生另一侧协议。
pub fn resolve_server_remote_endpoints(url: &str) -> Result<ServerRemoteEndpoints> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        bail!("server url is required");
    }
    let base = Url::parse(trimmed).with_context(|| format!("invalid server url: {trimmed}"))?;
    let mut info = base.clone();
    let mut ws = base.clone();
    let mut capability = base.clone();
    let mut host_ws = base.clone();
    match base.scheme() {
        "http" | "https" => {
            let ws_scheme = if base.scheme() == "https" {
                "wss"
            } else {
                "ws"
            };
            set_scheme(&mut ws, ws_scheme)?;
            set_scheme(&mut host_ws, ws_scheme)?;
        }
        "ws" | "wss" => {
            // 与 TS 一致：只有 info / capability 换协议；ws / hostWs 保持输入协议。
            let http_scheme = if base.scheme() == "wss" {
                "https"
            } else {
                "http"
            };
            set_scheme(&mut info, http_scheme)?;
            set_scheme(&mut capability, http_scheme)?;
        }
        other => bail!("unsupported server url protocol: {other}"),
    }
    set_endpoint(&mut info, INFO_PATH);
    set_endpoint(&mut ws, WS_PATH);
    set_endpoint(&mut capability, HOST_CAPABILITY_PATH);
    set_endpoint(&mut host_ws, HOST_WS_PATH);
    Ok(ServerRemoteEndpoints {
        info_url: info.to_string(),
        ws_url: ws.to_string(),
        host_capability_url: capability.to_string(),
        host_ws_url: host_ws.to_string(),
    })
}

/// 与 TS `authenticatedUrl` 一致：只有非空 token 才写入 `token` query（已存在则替换）。
pub fn authenticated_url(url: &str, token: Option<&str>) -> Result<String> {
    let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(url.to_owned());
    };
    let mut parsed = Url::parse(url).with_context(|| format!("invalid server url: {url}"))?;
    // 先摘出除 token 以外的参数，保证 `set` 语义而不是无脑追加。
    let retained: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| key != "token")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    parsed.set_query(None);
    {
        let mut pairs = parsed.query_pairs_mut();
        for (key, value) in retained {
            pairs.append_pair(&key, &value);
        }
        pairs.append_pair("token", token);
    }
    Ok(parsed.to_string())
}

fn set_scheme(url: &mut Url, scheme: &str) -> Result<()> {
    url.set_scheme(scheme)
        .map_err(|()| anyhow::anyhow!("cannot switch server url scheme to {scheme}"))
}

fn set_endpoint(url: &mut Url, endpoint: &str) {
    let base_path = strip_websocket_endpoint_path(url.path());
    url.set_path(&append_endpoint_path(&base_path, endpoint));
    url.set_query(None);
    url.set_fragment(None);
}

fn strip_websocket_endpoint_path(pathname: &str) -> String {
    let normalized = pathname.trim_end_matches('/');
    if let Some(rest) = normalized.strip_suffix(HOST_WS_PATH) {
        return rest.to_owned();
    }
    if let Some(rest) = normalized.strip_suffix(WS_PATH) {
        return rest.to_owned();
    }
    normalized.to_owned()
}

fn append_endpoint_path(base_path: &str, endpoint_path: &str) -> String {
    format!("{}{}", base_path.trim_end_matches('/'), endpoint_path)
}

#[cfg(test)]
#[path = "source_server_endpoints_tests.rs"]
mod tests;
