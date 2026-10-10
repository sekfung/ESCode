//! legacy `session/*` 请求（docs/specs/rust-legacy-session-methods.md）。
//!
//! 默认 runtime 切到 Rust 后，App 的 task facade、Bots、定时/闲时任务和 desktop session service 仍发
//! `session/create`（普通）、`session/send`、`session/setModel` 等旧请求，Rust 原先一律 `Unsupported method`。
//! 这里不新增写路径：把旧参数翻译成 V4 `Command`，走与 `v4/command` 相同的 `Engine::command`，
//! 再按 Node legacy 结果形状返回（快照与 `session/read` 同形）。
use super::Engine;
use crate::domain::protocol::Command;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

/// legacy 请求翻译出的 V4 命令的 clientId（ACK 账本与审计可区分来源）。
const LEGACY_CLIENT_ID: &str = "legacy-session";
const PERSISTENCE_DEFERRED: &str = "deferred";
const PERSISTENCE_IMMEDIATE: &str = "immediate";
/// legacy `session/send` 与 V4 sendText 的对应字段（旧名 → V4 名）。
const SEND_FIELDS: [(&str, &str); 8] = [
    ("modelSelection", "modelSelection"),
    ("modelExecution", "modelExecution"),
    ("automationId", "automationId"),
    ("offPeakTaskId", "offPeakTaskId"),
    ("offPeakRunType", "offPeakRunType"),
    ("botDeliveryTarget", "botDeliveryTarget"),
    ("browserAmbientContext", "browserAmbientContext"),
    ("toolDenylist", "toolDisallowlist"),
];

fn present<'a>(p: &'a Value, key: &str) -> Option<&'a Value> {
    p.get(key).filter(|v| !v.is_null())
}

fn nonempty<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p[key].as_str().filter(|s| !s.trim().is_empty())
}

impl Engine {
    fn legacy_command(
        &self,
        kind: &str,
        session: Option<&str>,
        payload: Value,
        command_id: Option<&str>,
        expected_revision: Option<u64>,
    ) -> Command {
        Command {
            command_id: command_id
                .map(str::to_owned)
                .unwrap_or_else(|| self.clock.id()),
            client_id: LEGACY_CLIENT_ID.into(),
            session_id: session.map(str::to_owned),
            base_revision: expected_revision,
            base_log_epoch: None,
            kind: kind.into(),
            payload,
            issued_at: self.clock.now() as f64,
        }
    }

    /// 走 V4 唯一写路径；拒绝 / stale 转成请求错误（legacy 没有 ACK 状态位，不能返回成功快照）。
    async fn legacy_dispatch(&mut self, command: Command) -> Result<Value> {
        let kind = command.kind.clone();
        let ack = self.command(command).await?;
        match ack["status"].as_str() {
            Some("accepted" | "duplicate" | "noop") => Ok(ack),
            status => bail!(
                "{kind} {}: {}",
                status.unwrap_or("failed"),
                ack["reasonCode"].as_str().unwrap_or("unknown")
            ),
        }
    }

    fn legacy_snapshot(&self, id: &str) -> Result<Value> {
        self.read_session(&json!({ "sessionId": id }))
    }

    /// 普通 `session/create`（无 importedHistory）：TS `createSession` 的 legacy 面。
    pub(super) async fn legacy_create(&mut self, p: &Value) -> Result<Value> {
        self.validate_workspace(p)?;
        ensure!(
            p.get("sessionId").is_none(),
            "sessionId is only supported for imported history creates"
        );
        ensure!(
            present(p, "parentSessionId").is_none(),
            "parentSessionId is not supported for legacy creates"
        );
        ensure!(
            ["toolAllowlist", "toolDenylist"]
                .iter()
                .all(|key| p[*key].as_array().is_none_or(Vec::is_empty)),
            "Unsupported legacy tool profile"
        );
        ensure!(
            p.get("titleGenerationEnabled").is_none_or(Value::is_boolean),
            "Invalid titleGenerationEnabled"
        );
        let mode = p["mode"].as_str();
        ensure!(
            mode.is_none_or(|m| matches!(m, "yolo" | "build" | "edit" | "plan")),
            "Unsupported core execution mode"
        );
        let persistence = p["persistence"].as_str().unwrap_or(PERSISTENCE_IMMEDIATE);
        ensure!(
            matches!(persistence, PERSISTENCE_IMMEDIATE | PERSISTENCE_DEFERRED),
            "Invalid persistence"
        );
        let mut config = json!({});
        if let Some(model) = present(p, "model") {
            config["provider"] = model["providerId"].clone();
            config["model"] = model["modelId"].clone();
            if let Some(level) = nonempty(&model["options"], "reasoningLevel") {
                config["thought"] = level.into();
            }
        }
        if let Some(level) = nonempty(p, "thoughtLevel") {
            let mut with_level = config.clone();
            with_level["thought"] = level.into();
            // TS：workspace 默认档位可能来自上一个模型；新模型不支持时跳过档位而不是创建失败。
            if self.select(&with_level, None).is_ok() {
                config = with_level;
            }
        }
        let mut payload = json!({ "workspaceId": self.workspace, "config": config });
        for key in ["mcpServers", "offPeakToolEnabled", "dynamicWorkflowEnabled"] {
            if let Some(value) = present(p, key) {
                payload[key] = value.clone();
            }
        }
        let command = self.legacy_command("createSession", None, payload, None, None);
        let ack = self.legacy_dispatch(command).await?;
        let id = ack["result"]["sessionId"]
            .as_str()
            .context("createSession accepted without sessionId")?
            .to_owned();
        if let Some(mode) = mode.filter(|m| *m != "build") {
            self.legacy_switch_mode(&id, mode, None).await?;
        }
        // persistence 只校验取值：Node 的空会话无论 immediate/deferred 都要到首条输入才进会话库
        // （差分实测：immediate 空会话重启后 resume 报 Session not found），Rust 草稿同语义。
        self.legacy_snapshot(&id)
    }

    async fn legacy_session(&mut self, p: &Value) -> Result<String> {
        let id = nonempty(p, "sessionId")
            .context("Session id required")?
            .to_owned();
        self.ensure_session(&id).await?;
        Ok(id)
    }

    pub(super) async fn legacy_set_model(&mut self, p: &Value) -> Result<Value> {
        let id = self.legacy_session(p).await?;
        let model = present(p, "model").context("model is required")?;
        let command = self.legacy_command(
            "switchModelConfig",
            Some(&id),
            json!({ "modelSelection": model }),
            None,
            p["expectedRevision"].as_u64(),
        );
        self.legacy_dispatch(command).await?;
        self.legacy_snapshot(&id)
    }

    pub(super) async fn legacy_set_thought_level(&mut self, p: &Value) -> Result<Value> {
        let level = nonempty(p, "thoughtLevel")
            .context("thoughtLevel is required")?
            .to_owned();
        let id = self.legacy_session(p).await?;
        let current = self.session_selection(&id)?;
        ensure!(
            !current.provider_id.is_empty() && !current.model_id.is_empty(),
            "Select a model before setting the thought level"
        );
        let command = self.legacy_command(
            "switchModelConfig",
            Some(&id),
            json!({ "provider": current.provider_id, "model": current.model_id, "thought": level }),
            None,
            p["expectedRevision"].as_u64(),
        );
        self.legacy_dispatch(command).await?;
        self.legacy_snapshot(&id)
    }

    pub(super) async fn legacy_set_mode(&mut self, p: &Value) -> Result<Value> {
        let id = self.legacy_session(p).await?;
        let mode = p["mode"].as_str().context("mode is required")?;
        self.legacy_switch_mode(&id, mode, p["expectedRevision"].as_u64())
            .await?;
        self.legacy_snapshot(&id)
    }

    /// legacy mode 是含 plan 的单一枚举；V4 用 mode + 独立 planEnabled 表达。auto 不在 Rust 声明的能力里。
    async fn legacy_switch_mode(
        &mut self,
        id: &str,
        mode: &str,
        expected_revision: Option<u64>,
    ) -> Result<()> {
        let payload = match mode {
            "plan" => json!({ "planEnabled": true }),
            "yolo" | "build" | "edit" => json!({ "mode": mode, "planEnabled": false }),
            _ => bail!("Unsupported core execution mode"),
        };
        let command = self.legacy_command(
            "switchCollaborationMode",
            Some(id),
            payload,
            None,
            expected_revision,
        );
        self.legacy_dispatch(command).await?;
        Ok(())
    }

    /// `session/close`：关闭 runtime、保留历史（同 V4 deleteSession）；`expectedPersistence` 是原子条件。
    pub(super) async fn legacy_close(&mut self, p: &Value) -> Result<Value> {
        let id = nonempty(p, "sessionId")
            .context("Session id required")?
            .to_owned();
        let expected = p["expectedPersistence"].as_str();
        ensure!(
            expected.is_none_or(|e| matches!(e, PERSISTENCE_IMMEDIATE | PERSISTENCE_DEFERRED)),
            "Invalid expectedPersistence"
        );
        let Some(session) = self.sessions.get(&id) else {
            // 没有 runtime 可关；未加载的会话必然已持久化，按条件判定是否「关闭成立」。
            return Ok(json!({ "closed": expected != Some(PERSISTENCE_DEFERRED) }));
        };
        let current = if session.phase == "draft" {
            PERSISTENCE_DEFERRED
        } else {
            PERSISTENCE_IMMEDIATE
        };
        if expected.is_some_and(|e| e != current) {
            // TS：并发首发可能已把草稿提升为正式会话，条件关闭必须在 runtime 侧原子判断。
            return Ok(json!({ "closed": false }));
        }
        let command = self.legacy_command("deleteSession", Some(&id), json!({}), None, None);
        self.legacy_dispatch(command).await?;
        Ok(json!({ "closed": true }))
    }

    /// `session/send`：legacy 输入（含旧附件载荷）。legacy 不排队，运行中直接拒绝（TS 同文）。
    pub(super) async fn legacy_send(&mut self, p: &Value) -> Result<Value> {
        let id = self.legacy_session(p).await?;
        ensure!(
            !self.sessions[&id].running(),
            "A prompt is already running for this session"
        );
        let content = p["content"].as_str().context("content is required")?;
        let mut payload = json!({
            "text": content,
            "requestedDelivery": "startNow",
            // 旧 session/send 没有 held 队列闸门：立即发送、不动队列（与 task facade 的 V4 主路径一致）。
            "heldQueueDisposition": "keepQueueAndSend",
        });
        for (from, to) in SEND_FIELDS {
            if let Some(value) = present(p, from) {
                payload[to] = value.clone();
            }
        }
        if let Some(items) = p["attachments"].as_array().filter(|a| !a.is_empty()) {
            payload["attachments"] = self.legacy_attachments(&id, items).await?.into();
        }
        let command = self.legacy_command(
            "sendText",
            Some(&id),
            payload,
            nonempty(p, "inputId"),
            p["expectedRevision"].as_u64(),
        );
        self.legacy_dispatch(command).await?;
        Ok(json!({
            "sessionId": id,
            "accepted": true,
            "stateRevision": self.sessions.get(&id).map_or(0, |s| s.revision),
        }))
    }

    /// 旧附件（kind/filename/mimeType + localPath | dataBase64 | textContent）转成会话内的 V4 attachmentRef。
    /// 内容先进附件存储再登记到会话（同 `v4/attachment/commit`），之后由普通 admission 校验能力与 MIME。
    async fn legacy_attachments(&mut self, id: &str, items: &[Value]) -> Result<Vec<Value>> {
        use base64::Engine as _;
        let mut refs = Vec::with_capacity(items.len());
        for item in items {
            let mime = nonempty(item, "mimeType")
                .context("Attachment mimeType required")?
                .to_ascii_lowercase();
            let name = nonempty(item, "filename").context("Attachment filename required")?;
            // TS：有 localPath 时由 runtime 自行读取；无路径时回退 base64 / 小文本正文。
            let asset = if let Some(path) = nonempty(item, "localPath") {
                self.store.snapshot_attachment(path, &mime).await?
            } else if let Some(data) = nonempty(item, "dataBase64") {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .context("Invalid attachment base64")?;
                self.store.put_attachment(&[bytes], &mime).await?
            } else if let Some(text) = item["textContent"].as_str() {
                self.store
                    .put_attachment(&[text.as_bytes().to_vec()], &mime)
                    .await?
            } else {
                bail!("Attachment content required");
            };
            let reference = format!("escode-artifact://{id}/{}", self.clock.id());
            let bytes = asset.total_bytes;
            self.sessions
                .get_mut(id)
                .context("Session unavailable")?
                .attachments
                .insert(reference.clone(), asset);
            refs.push(json!({ "ref": reference, "fileName": name, "mime": mime, "bytes": bytes }));
        }
        Ok(refs)
    }
}
