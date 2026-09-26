use super::mcp_config::Server;
use crate::contract::{ProcessCleanupFailure, ToolOutput};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use rmcp::{
    RoleClient,
    model::{ClientConfig, ClientRequest, ProtocolVersion},
    service::{
        ClientLifecycleMode, Peer, PeerRequestOptions, RunningService,
        serve_client_with_lifecycle_and_ct,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};
use tokio_util::{
    codec::{FramedRead, FramedWrite},
    sync::CancellationToken,
};

type Service = RunningService<RoleClient, ClientConfig>;
pub(super) struct Connection {
    pub peer: Peer<RoleClient>,
    pub tools: Vec<Value>,
    pub modern: bool,
    timeout: Duration,
    service: Mutex<Option<Service>>,
    child: Mutex<Option<(Child, u32)>>,
}
impl Connection {
    pub async fn open(
        config: &Server,
        http: Option<reqwest_mcp::Client>,
        oauth: Option<super::mcp_oauth_credentials::Auth>,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        use super::mcp_oauth_credentials::Auth;
        let official = match &oauth {
            Some(Auth::Official(official)) => Some(official.clone()),
            _ => None,
        };
        // 非官方 HTTP/SSE 一律经 AuthClient（无鉴权时为 Plain），统一 401 处理与关闭语义（不发 DELETE）。
        let auth = http.clone().map(|http| {
            let oauth = oauth.clone().filter(|a| !matches!(a, Auth::Official(_)));
            super::mcp_oauth_client::AuthClient::new(http, oauth.unwrap_or(Auth::Plain))
        });
        let official_failure = || -> Option<anyhow::Error> {
            Some(official.as_ref()?.connect_failure()?.into())
        };
        // TS resolveVersionNegotiationMode：未写或 auto 时协商（先 server/discover），SSE 只承载 legacy；
        // 之前未写时直接 initialize，与 Node 的默认请求序列不同（docs/specs/rust-mcp-parity.md「协议协商默认值」）。
        let mode = match config.raw["protocolVersion"].as_str() {
            Some("2026-07-28") => ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            _ if config.transport == "sse" => ClientLifecycleMode::Initialize,
            Some("legacy") => ClientLifecycleMode::Initialize,
            _ => ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::LATEST),
            },
        };
        let mut client = ClientConfig::default();
        client.client_info.name = "zcode-cli-rust".into();
        client.client_info.version = env!("CARGO_PKG_VERSION").into();
        let lifecycle = CancellationToken::new();
        let mut owned = None;
        let init = async {
            if config.transport == "stdio" {
                let mut command = Command::new(config.raw["command"].as_str().unwrap());
                command
                    .args(super::extension_config::strings(&config.raw["args"]))
                    .current_dir(&config.cwd)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                #[cfg(unix)]
                command.process_group(0);
                if let Some(env) = config.raw["env"].as_object() {
                    command.envs(env.iter().map(|(k, v)| (k, v.as_str().unwrap())));
                }
                let mut child = command.spawn().context("process_start_failed")?;
                let pid = child.id().context("process_start_failed")?;
                let input = child.stdin.take().unwrap();
                let output = child.stdout.take().unwrap();
                owned = Some((child, pid));
                // SDK 默认读行无界；使用有界 codec，坏帧立即断开而非无限缓存或跳过。
                let reader = FramedRead::new(
                    output,
                    rmcp::transport::async_rw::JsonRpcMessageCodec::<
                        rmcp::model::ServerJsonRpcMessage,
                    >::new_with_max_length(8 * 1024 * 1024),
                )
                .take_while(|r| std::future::ready(r.is_ok()))
                .filter_map(|r| std::future::ready(r.ok()));
                let writer = FramedWrite::new(
                    input,
                    rmcp::transport::async_rw::JsonRpcMessageCodec::<Value>::new_with_max_length(
                        8 * 1024 * 1024,
                    ),
                );
                let meta = official.clone();
                let writer = futures_util::SinkExt::with(
                    writer,
                    move |message: rmcp::model::ClientJsonRpcMessage| {
                        // SinkStream 传输要求 Unpin：异步取身份载荷的 future 需装箱。
                        Box::pin(super::mcp_official_stdio::with_meta(meta.clone(), message))
                    },
                );
                Ok::<Service, anyhow::Error>(
                    serve_client_with_lifecycle_and_ct(
                        client,
                        (writer, reader),
                        mode,
                        lifecycle.clone(),
                    )
                    .await
                    .context("protocol_negotiation_failed")?,
                )
            } else if config.transport == "sse" {
                let transport = super::mcp_sse::Transport::open(
                    config,
                    http.context("HTTP client missing")?,
                    auth.clone(),
                    cancel,
                )
                .await?;
                Ok(
                    serve_client_with_lifecycle_and_ct(client, transport, mode, lifecycle.clone())
                        .await
                        .context("protocol_negotiation_failed")?,
                )
            } else {
                use rmcp::transport::{
                    StreamableHttpClientTransport,
                    streamable_http_client::StreamableHttpClientTransportConfig,
                };
                let mut options = StreamableHttpClientTransportConfig::with_uri(
                    config.raw["url"].as_str().unwrap().to_owned(),
                );
                options.max_sse_event_size = 8 * 1024 * 1024;
                options.reinit_on_expired_session = false;
                options.max_concurrent_requests = 4;
                options.control_request_timeout = Duration::from_secs(2);
                if let Some(headers) = config.raw["headers"].as_object() {
                    for (name, value) in headers {
                        options.custom_headers.insert(
                            name.parse().context("config_invalid")?,
                            value.as_str().unwrap().parse().context("config_invalid")?,
                        );
                    }
                }
                let http = http.context("HTTP client missing")?;
                let service = match (auth.clone(), official.clone()) {
                    (_, Some(official)) => {
                        let official_client = super::mcp_official_client::OfficialClient::new(http, official);
                        let transport = StreamableHttpClientTransport::with_client(official_client, options);
                        serve_client_with_lifecycle_and_ct(client, transport, mode, lifecycle.clone()).await
                    }
                    (Some(auth), None) => {
                        let transport = StreamableHttpClientTransport::with_client(auth, options);
                        serve_client_with_lifecycle_and_ct(client, transport, mode, lifecycle.clone()).await
                    }
                    (None, None) => {
                        let transport = StreamableHttpClientTransport::with_client(http, options);
                        serve_client_with_lifecycle_and_ct(client, transport, mode, lifecycle.clone()).await
                    }
                };
                Ok(service.context("protocol_negotiation_failed")?)
            }
        };
        let result = tokio::select! {biased; _=cancel.cancelled()=>Err(anyhow::anyhow!("Cancelled")), result=tokio::time::timeout(config.timeout,init)=>result.unwrap_or_else(|_|Err(anyhow::anyhow!("connection_timeout")))};
        let service = match result {
            Ok(service) => service,
            Err(error) => {
                lifecycle.cancel();
                cleanup_child(&mut owned).await?;
                // 需要交互授权时以分类结果替换握手错误，交给 hub 发起授权（TS openServerConnection catch 分支）。
                if let Some(super::mcp_oauth_client::Failure::Required(required)) =
                    auth.as_ref().and_then(|a| a.take_failure())
                {
                    return Err(required.into());
                }
                return Err(official_failure().unwrap_or(error));
            }
        };
        let modern = service
            .peer_info()
            .is_some_and(|i| i.protocol_version >= ProtocolVersion::V_2026_07_28);
        let mut connection = Self {
            peer: service.peer().clone(),
            tools: vec![],
            modern,
            timeout: config.timeout,
            service: Mutex::new(Some(service)),
            child: Mutex::new(owned),
        };
        match connection.discover(cancel).await {
            Ok(tools) => connection.tools = tools,
            Err(error) => {
                connection.close().await?;
                return Err(official_failure().unwrap_or_else(|| error.context("tool_list_failed")));
            }
        }
        Ok(connection)
    }
    async fn discover(&self, cancel: &CancellationToken) -> Result<Vec<Value>> {
        let mut tools = vec![];
        let mut cursor = Value::Null;
        let mut seen = BTreeSet::new();
        for _ in 0..128 {
            let params = if cursor.is_null() {
                json!({})
            } else {
                json!({"cursor":cursor})
            };
            let result = self.request("tools/list", params, cancel).await?;
            tools.extend(
                result["tools"]
                    .as_array()
                    .context("Invalid MCP tools list")?
                    .iter()
                    .cloned(),
            );
            ensure!(tools.len() <= 10_000, "MCP tool count exceeded");
            cursor = result["nextCursor"].clone();
            if cursor.is_null() {
                return Ok(tools);
            }
            ensure!(
                cursor.is_string() && seen.insert(cursor.to_string()),
                "MCP pagination loop"
            );
        }
        bail!("MCP tool pages exceeded")
    }
    async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        Ok(serde_json::from_str(
            &self.request_text(method, params, cancel).await?,
        )?)
    }
    /// 应答的 JSON 文本（按 rmcp 类型的字段顺序），供需要保持键序的格式化使用。
    async fn request_text(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let request: ClientRequest =
            serde_json::from_value(json!({"method":method,"params":params}))?;
        let handle = tokio::select! {biased;_=cancel.cancelled()=>bail!("Cancelled"),result=self.peer.send_cancellable_request(request,PeerRequestOptions::with_timeout(self.timeout))=>result.context("MCP request dispatch failed")?};
        let id = handle.id.clone();
        let result = tokio::select! {biased;
            _=cancel.cancelled()=>{
                let _=tokio::time::timeout(Duration::from_secs(2),self.peer.notify_cancelled(rmcp::model::CancelledNotificationParam::new(Some(id),Some("Cancelled".into())))).await;
                Err(anyhow::anyhow!("Cancelled"))
            },
            result=handle.await_response()=>result.map_err(|_|anyhow::anyhow!("MCP request failed or timed out")),
        };
        if result.is_err() && method == "tools/call" {
            self.close().await?;
        }
        Ok(serde_json::to_string(&result?)?)
    }
    pub async fn call(
        &self,
        name: &str,
        args: &Value,
        meta: &Value,
        artifacts: Option<ImageArtifacts<'_>>,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput> {
        let mut params = json!({"name":name,"arguments":args});
        if let Some(meta) = super::mcp_request_meta::request_meta(meta) {
            params["_meta"] = meta;
        }
        let text = self.request_text("tools/call", params, cancel).await?;
        let result: Value = serde_json::from_str(&text)?;
        let ordered = crate::domain::json_order::Json::parse(&text);
        // 修复：此前超过 inline 预算的图片一律给出「无 artifact store」说明；App 中 TS 有 artifact store，
        // 会写二进制 artifact 并告知模型路径与 URI（docs/specs/rust-mcp-parity.md 第 2 期）。
        let mut saved = crate::domain::mcp_result::SavedImages::new();
        if let Some(target) = artifacts {
            for (index, mime, payload) in crate::domain::mcp_result::oversized_images(&result) {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD.decode(payload.trim())?;
                saved.insert(index, target.write(&mime, &bytes).await?);
            }
        }
        // 修复：此前按换行拼接文本、图片/音频/resource 整块 JSON 化且无错误前缀；
        // 按 TS formatMcpToolResult 生成模型内容（docs/specs/rust-mcp-parity.md）。
        let formatted = crate::domain::mcp_result::format(&result, ordered.as_ref(), &saved);
        let mut content = formatted.text;
        if content.len() > crate::domain::MAX_TOOL_BYTES {
            super::tools::truncate_utf8(&mut content, crate::domain::MAX_TOOL_BYTES);
            content.push_str("\n[MCP result truncated]");
        }
        let mut output = ToolOutput::text(content);
        output.media = formatted.media;
        output.failed = result["isError"] == true;
        Ok(output)
    }
    pub async fn close(&self) -> Result<()> {
        if let Some(mut service) = self.service.lock().await.take() {
            // 先断协议再等待进程树；不能只 drop transport 后宣告停止完成。
            let mut child = self.child.lock().await.take();
            let (protocol, process) = tokio::join!(service.close(), cleanup_child(&mut child));
            process?;
            protocol.context(ProcessCleanupFailure)?;
        }
        Ok(())
    }
}
async fn cleanup_child(child: &mut Option<(Child, u32)>) -> Result<()> {
    if let Some((mut child, pid)) = child.take()
        && let Err(error) = super::tool_process::terminate(&mut child, pid, true).await
    {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr().lock(),
            "MCP process cleanup failed: {error:#}"
        );
        return Err(error.context(ProcessCleanupFailure));
    }
    Ok(())
}

/// MCP 超预算图片的二进制 artifact 目标（TS `writeToolResultBinaryArtifact`）：
/// `<root>/<session>/<toolCallId>-tool-result-<uuid><ext>`，URI `zcode-artifact://<session>/tool-result-<uuid>`。
#[derive(Clone, Copy)]
pub struct ImageArtifacts<'a> {
    pub root: &'a std::path::Path,
    pub session: &'a str,
    pub call_id: &'a str,
}
/// TS `sanitizePathSegment`（存储版）：非 `[A-Za-z0-9._-]` 换成 `_`，最长 120，空串为 unknown。
fn sanitize(value: &str) -> String {
    let out: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    if out.is_empty() {
        "unknown".to_owned()
    } else {
        out
    }
}
impl ImageArtifacts<'_> {
    async fn write(&self, mime: &str, bytes: &[u8]) -> Result<(String, String)> {
        let artifact = format!("tool-result-{}", zcode_cli_host::id());
        let dir = self.root.join(sanitize(self.session));
        let path = dir.join(format!(
            "{}-{artifact}{}",
            sanitize(self.call_id),
            crate::domain::mcp_result::image_extension(mime)
        ));
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(&path, bytes).await?;
        Ok((
            path.to_string_lossy().into_owned(),
            format!("zcode-artifact://{}/{artifact}", self.session),
        ))
    }
}

