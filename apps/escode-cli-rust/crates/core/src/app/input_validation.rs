use super::Engine;
use crate::domain::MAX_TOOL_BYTES;
use anyhow::{Context, Result, bail};
use serde_json::Value;

impl Engine {
    pub(super) fn validate_selection(&self, p: &Value) -> Result<()> {
        // yolo/build/edit 与独立 plan 状态（planEnabled）已实现；mode 取值 plan/auto 仍拒绝
        // （TS 以 planEnabled 表达 plan，auto 同样保留未实现），显式拒绝而不是静默忽略。
        if p["mode"]
            .as_str()
            .is_some_and(|mode| !matches!(mode, "yolo" | "build" | "edit"))
            || p["followupMode"]
                .as_str()
                .is_some_and(|mode| !matches!(mode, "queue" | "guide"))
        {
            bail!("Unsupported core execution mode");
        }
        Ok(())
    }
    pub(super) fn validate_input(&self, p: &Value) -> Result<()> {
        self.validate_selection(p)?;
        if p.get("requestedDelivery")
            .is_some_and(|v| !matches!(v.as_str(), Some("auto" | "startNow" | "queue" | "guide")))
        {
            bail!("Invalid requested delivery");
        }
        // Composer 允许仅共享上下文发送；引用先严格解析，所属会话和状态仍由 admission 校验。
        let shared = crate::domain::shared_context::reference(p)?;
        p["text"]
            .as_str()
            .filter(|s| {
                s.len() <= MAX_TOOL_BYTES
                    && (!s.trim().is_empty()
                        || p["attachments"].as_array().is_some_and(|a| !a.is_empty())
                        || shared.is_some())
            })
            .context("Invalid input text")?;
        if let Some(refs) = p.get("attachments") {
            let refs = refs
                .as_array()
                .filter(|a| a.len() <= 16)
                .context("Invalid attachments")?;
            for item in refs {
                let attachment: crate::domain::protocol::AttachmentRef =
                    serde_json::from_value(item.clone())?;
                anyhow::ensure!(
                    !attachment.r#ref.is_empty()
                        && !attachment.r#ref.contains('\0')
                        && !attachment.file_name.is_empty()
                        && attachment.file_name.chars().count() <= 255
                        && !attachment.file_name.contains(['\0', '\r', '\n'])
                        && crate::domain::attachment_upload::valid_mime(&attachment.mime),
                    "Invalid attachment metadata"
                );
            }
        }
        // Cron 定时任务与闲时任务派发的本轮事实（docs/specs/rust-cron.md、rust-offpeak.md）。
        if p.get("toolDisallowlist").is_some_and(|v| {
            v.as_array()
                .is_none_or(|a| a.iter().any(|t| t.as_str().is_none_or(str::is_empty)))
        }) {
            bail!("Invalid toolDisallowlist");
        }
        if p.get("automationId")
            .is_some_and(|v| v.as_str().is_none_or(str::is_empty))
        {
            bail!("Invalid automationId");
        }
        if p.get("botDeliveryTarget").is_some_and(|v| !v.is_object()) {
            bail!("Invalid botDeliveryTarget");
        }
        if p.get("offPeakTaskId").is_some_and(|v| v.as_str().is_none_or(|s| s.trim().is_empty())) {
            bail!("Invalid offPeakTaskId");
        }
        if p.get("offPeakRunType").is_some_and(|v| !matches!(v.as_str(), Some("init" | "resume"))) {
            bail!("Invalid offPeakRunType");
        }
        // 协议 superRefine：两类派发身份互斥；runType 需要闲时身份。
        if p.get("automationId").is_some() && p.get("offPeakTaskId").is_some() {
            bail!("automationId and offPeakTaskId are mutually exclusive");
        }
        if p.get("offPeakRunType").is_some() && p.get("offPeakTaskId").is_none() {
            bail!("offPeakRunType requires offPeakTaskId");
        }
        // 单次执行约束（docs/specs/rust-offpeak.md 第三期）：strict 形态，且需要同时给出 modelSelection。
        if let Some(execution) = p.get("modelExecution") {
            crate::domain::model_execution::parse(execution).map_err(anyhow::Error::msg)?;
            if p.get("modelSelection").is_none_or(Value::is_null) {
                bail!("modelExecution requires modelSelection");
            }
        }
        // 协议 escodeBrowserAmbientContextSchema（strict）：tabCount 为 1..=100 的整数，currentUrl 非空且 ≤4096。
        if let Some(ambient) = p.get("browserAmbientContext") {
            let valid = ambient.as_object().is_some_and(|o| {
                o.keys().all(|k| k == "tabCount" || k == "currentUrl")
                    && o.get("tabCount").and_then(Value::as_u64).is_some_and(|c| (1..=100).contains(&c))
                    && o.get("currentUrl").is_none_or(|u| {
                        u.as_str().is_some_and(|u| !u.trim().is_empty() && u.trim().chars().count() <= 4096)
                    })
            });
            if !valid {
                bail!("Invalid browserAmbientContext");
            }
        }
        if p.get("heldQueueDisposition")
            .is_some_and(|v| !matches!(v.as_str(), Some("clearQueueAndSend" | "keepQueueAndSend")))
        {
            bail!("Invalid held queue disposition");
        }
        if p.get("expectedHeldQueueItemIds").is_some_and(|v| {
            v.as_array().is_none_or(|a| {
                a.len() > crate::domain::MAX_QUEUE
                    || a.iter().any(|id| id.as_str().is_none_or(str::is_empty))
            })
        }) {
            bail!("Invalid expected queue ids");
        }
        Ok(())
    }
}
