//! V4 作用域：命令、命令查询、只读 query 族与附件事务（TS `overrides` + base 转发）。

use serde_json::{Map, Value, json};

use super::base::{self, pick};
use super::scope::{Scope, fault, flow_key, workspace_key, workspace_target};
use crate::forward::{RUNTIME_UNAVAILABLE_CODE, RuntimePolicy};
use crate::session::{ClientMode, FlowState, RpcError};

impl Scope {
    /// `sendConversationCommandV4`。
    pub(crate) async fn send_command(&self, params: &Value) -> Result<Value, RpcError> {
        self.assert_open()?;
        self.assert_ready()?;
        let mut envelope = params.get("envelope").cloned().unwrap_or(Value::Null);
        let bound = self.lock().bound_client_id.clone();
        if envelope["clientId"].as_str() != bound.as_deref() {
            // 未握手调用与伪造 clientId 都会直达 CLI；静默覆盖又会破坏命令幂等归属，故明确拒绝。
            return Err(fault("fault.command.clientMismatch"));
        }
        let payload = &envelope["payload"];
        if payload["planEnabled"] == true
            || payload["config"]["planEnabled"] == true
            || payload["firstInput"]["planEnabled"] == true
        {
            self.base.ensure_plan_support(params).await?;
        }
        // 工具面灰度：Host 判定（当前关闭）→ createSession 信封不注入任何 flag（TS 门禁 false 时原样返回）。
        // TTFT 只允许可信桌面本地 continuous。
        let local_desktop = self.client_mode == ClientMode::DesktopContinuous
            && params["workspaceIdentity"]
                .as_str()
                .is_none_or(|v| v.trim().is_empty())
            && !base::is_truthy(&params["remoteSessionId"]);
        if !local_desktop && let Some(record) = envelope.as_object_mut() {
            record.remove("ttft");
        }
        if envelope["type"] == "sendText"
            && base::is_truthy(&envelope["sessionId"])
            && let Some(payload) = envelope["payload"].as_object_mut()
        {
            let origin = if self.client_mode == ClientMode::WebRemoteReplayable {
                "mobile"
            } else {
                "desktop"
            };
            payload.insert("inputOrigin".to_owned(), Value::from(origin));
        }
        let called = self
            .base
            .call(params, RuntimePolicy::StartIfNeeded, "v4/command", envelope)
            .await?;
        Ok(called.result)
    }

    /// `queryConversationCommandsV4`：纯时钟探测不绑定 workspace；业务查询只允许一个 workspace。
    pub(crate) async fn query_commands(&self, params: &Value) -> Result<Value, RpcError> {
        self.assert_ready()?;
        let clock = base::is_truthy(&params["clock"]);
        if !clock {
            let requested = workspace_key(params);
            let mut state = self.lock();
            if state
                .command_query_workspace
                .as_ref()
                .is_some_and(|bound| *bound != requested)
            {
                return Err(fault("fault.command.queryForeignWorkspace"));
            }
            state.command_query_workspace = Some(requested);
        }
        let mut body = Map::new();
        body.insert(
            "commands".to_owned(),
            params.get("commands").cloned().unwrap_or(Value::Null),
        );
        if clock {
            body.insert("clock".to_owned(), Value::Bool(true));
        }
        let called = self
            .base
            .call(
                params,
                RuntimePolicy::StartIfNeeded,
                "v4/commands/query",
                Value::Object(body),
            )
            .await?;
        Ok(called.result)
    }

    /// `backgroundBashOutputV4`：existing-only；运行时不认方法 → unsupported，无运行时 → unavailable。
    pub(crate) async fn background_bash_output(&self, params: &Value) -> Result<Value, RpcError> {
        self.assert_ready()?;
        let session_id = params["sessionId"].as_str().filter(|v| !v.is_empty());
        let work_id = params["workId"].as_str().filter(|v| !v.is_empty());
        let (Some(session_id), Some(work_id)) = (session_id, work_id) else {
            return Err(RpcError::new(
                "ZodError",
                "Invalid background bash output params",
            ));
        };
        let body = json!({"sessionId": session_id, "workId": work_id});
        match self
            .base
            .call(
                params,
                RuntimePolicy::ExistingOnly,
                "v4/conversation/backgroundBashOutput",
                body,
            )
            .await
        {
            Ok(called) => Ok(called.result),
            Err(error) if error.extra.get("code") == Some(&Value::from(-32601)) => {
                Ok(json!({"kind": "unsupported", "workId": work_id}))
            }
            Err(error)
                if error.extra.get("code") == Some(&Value::from(RUNTIME_UNAVAILABLE_CODE)) =>
            {
                Ok(json!({"kind": "unavailable", "workId": work_id}))
            }
            Err(error) => Err(error),
        }
    }

    /// 只读 query 族与附件读（请求体按 TS 字面量）。
    pub(crate) async fn query(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let (runtime_method, body, ready) = query_body(method, params, self)?;
        if ready {
            self.assert_ready()?;
        }
        let called = self
            .base
            .call(params, RuntimePolicy::StartIfNeeded, runtime_method, body)
            .await?;
        Ok(called.result)
    }

    /// `attachmentPreviewSourceV4`：只有可信桌面本地连接拿本地播放源，其余一律分块读取。
    pub(crate) async fn preview_source(&self, params: &Value) -> Result<Value, RpcError> {
        self.assert_ready()?;
        if self.client_mode != ClientMode::DesktopContinuous
            || base::is_truthy(&params["remoteSessionId"])
        {
            return Ok(json!({"kind": "chunked"}));
        }
        let mut body = pick(params, &["sessionId", "ref", "target", "attachmentIndex"]);
        body.insert(
            "clientMode".to_owned(),
            Value::from(self.client_mode.as_str()),
        );
        let called = self
            .base
            .call(
                params,
                RuntimePolicy::StartIfNeeded,
                "v4/attachment/previewSource",
                Value::Object(body),
            )
            .await?;
        if called.result["kind"] != "local_path" {
            return Ok(called.result);
        }
        // 本地媒体协议授权（TS `authorizeLocalMediaPreviewPath`，属 media-preview 域）未迁移：与 TS 缺授权器时同错。
        Err(fault(
            "fault.attachment.previewPathAuthorizationUnavailable",
        ))
    }

    /// 附件事务（begin/chunk/commit/abort）。
    pub(crate) async fn attachment(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        self.assert_ready()?;
        let keys: &[&str] = match method {
            "attachmentBeginV4" => &[
                "uploadId",
                "sessionId",
                "fileName",
                "mime",
                "totalBytes",
                "totalChunks",
                "checksum",
            ],
            "attachmentChunkV4" => &["uploadId", "sessionId", "chunkIndex", "dataBase64"],
            _ => &["uploadId", "sessionId"],
        };
        let runtime_method = match method {
            "attachmentBeginV4" => "v4/attachment/begin",
            "attachmentChunkV4" => "v4/attachment/chunk",
            "attachmentCommitV4" => "v4/attachment/commit",
            _ => "v4/attachment/abort",
        };
        let mut body = Map::new();
        body.insert(
            "connectionId".to_owned(),
            Value::from(self.connection_id.clone()),
        );
        body.extend(pick(params, keys));
        let body = Value::Object(body);
        base::assert_attachment_envelope(runtime_method, &body)?;
        let called = self
            .base
            .call(params, RuntimePolicy::StartIfNeeded, runtime_method, body)
            .await?;
        if method == "attachmentBeginV4" && called.result["state"] == "staging" {
            self.register_attachment_route(params).await?;
        }
        if method == "attachmentAbortV4" {
            return Ok(Value::Null);
        }
        Ok(called.result)
    }

    /// begin ACK 后登记 attachment 的 flow 路由；若已关闭或补发 SAT 失败则用同一可信身份 abort。
    async fn register_attachment_route(&self, params: &Value) -> Result<(), RpcError> {
        let key = flow_key(&workspace_key(params), &self.connection_id);
        let (closed, saturated) = {
            let state = self.lock();
            (
                state.disposed || state.flow_closed,
                state.current_flow == Some(FlowState::Saturated),
            )
        };
        let abort = || async {
            let body = json!({
                "connectionId": self.connection_id,
                "uploadId": params["uploadId"],
                "sessionId": params["sessionId"],
            });
            let _ = self
                .base
                .call(
                    params,
                    RuntimePolicy::StartIfNeeded,
                    "v4/attachment/abort",
                    body,
                )
                .await;
        };
        if closed {
            abort().await;
            return Err(fault("fault.connection.closed"));
        }
        let target = workspace_target(params);
        self.lock()
            .attachment_routes
            .insert(key.clone(), target.clone());
        if saturated
            && let Err(error) = self.forward_flow(&key, &target, FlowState::Saturated).await
        {
            self.lock().attachment_routes.remove(&key);
            abort().await;
            return Err(error);
        }
        Ok(())
    }
}

/// 只读 query 族的 (runtime 方法, 请求体, 是否要求握手)。
fn query_body(
    method: &str,
    params: &Value,
    scope: &Scope,
) -> Result<(&'static str, Value, bool), RpcError> {
    let body = |keys: &[&str]| Value::Object(pick(params, keys));
    Ok(match method {
        "conversationRowsRangeV4" => {
            let mut record = pick(params, &["sessionId"]);
            record.insert(
                "clientMode".to_owned(),
                Value::from(scope.client_mode.as_str()),
            );
            record.extend(pick(params, &["beforeRowId", "limit"]));
            ("v4/conversation/rowsRange", Value::Object(record), true)
        }
        "conversationPlansV4" => ("v4/conversation/plans", body(&["sessionId"]), false),
        "conversationWorkflowRunEventsV4" => (
            "v4/conversation/workflowRunEvents",
            body(&["sessionId", "runId", "afterSequence", "limit"]),
            false,
        ),
        "conversationWorkflowRunsV4" => (
            "v4/conversation/workflowRuns",
            body(&["sessionId", "limit"]),
            false,
        ),
        "conversationWorkflowRunArtifactsV4" => (
            "v4/conversation/workflowRunArtifacts",
            body(&["sessionId", "runId"]),
            false,
        ),
        "conversationWorkflowRunArtifactDataV4" => (
            "v4/conversation/workflowRunArtifactData",
            body(&["sessionId", "runId", "artifactId", "afterSequence", "limit"]),
            false,
        ),
        "conversationWorkflowRunArtifactReadV4" => (
            "v4/conversation/workflowRunArtifactRead",
            body(&[
                "sessionId",
                "runId",
                "artifactId",
                "version",
                "offset",
                "limit",
            ]),
            false,
        ),
        "conversationWorkflowRunWorkspaceV4" => (
            "v4/conversation/workflowRunWorkspace",
            body(&["sessionId", "runId"]),
            false,
        ),
        "conversationWorkflowRunNodeResultV4" => (
            "v4/conversation/workflowRunNodeResult",
            body(&["sessionId", "runId", "siteId", "ordinal", "maxBytes"]),
            false,
        ),
        "conversationFileChangesV4" => (
            "v4/conversation/fileChanges",
            body(&["sessionId", "target", "baseRevision", "baseLogEpoch"]),
            false,
        ),
        "conversationFileRewindPreviewV4" => (
            "v4/conversation/fileRewindPreview",
            body(&["sessionId", "target", "baseRevision", "baseLogEpoch"]),
            false,
        ),
        "attachmentReadV4" => (
            "v4/attachment/read",
            body(&[
                "sessionId",
                "ref",
                "target",
                "attachmentIndex",
                "offset",
                "limit",
            ]),
            true,
        ),
        "conversationAttachmentReadV4" => (
            "v4/conversation/attachmentRead",
            body(&[
                "sessionId",
                "ref",
                "target",
                "attachmentIndex",
                "offset",
                "limit",
            ]),
            true,
        ),
        "conversationAttachmentStatV4" => (
            "v4/conversation/attachmentStat",
            body(&["sessionId", "ref", "target", "attachmentIndex"]),
            true,
        ),
        other => {
            return Err(RpcError::new(
                "NotImplemented",
                format!("Host does not implement `escode-agent.{other}` yet"),
            ));
        }
    })
}
