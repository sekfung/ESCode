//! OffPeak 工具面与执行（docs/specs/rust-offpeak.md 第一期）：守卫与参数由 domain::off_peak 决定，
//! Host 反向请求经会话 owner 转发。工具面开关按 TS 读法：会话创建参数优先，缺席时读进程级 workspace 结论；
//! 会话在创建/首次使用时固化，之后 workspace 结论变化不回收已固化的会话（TS「已活跃 record 不回收」）。

use super::Engine;
use crate::contract::{Event, EventSink, ToolOutput};
use crate::domain::cron::HostError;
use crate::domain::json_order::Json;
use crate::domain::off_peak::{self, Turn};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

impl Engine {
    /// `workspace/updateOffPeakToolPolicy`（strict `{workspace, enabled}`）：之前已使用过的会话按旧结论固化。
    pub(super) fn update_off_peak_policy(&mut self, p: &Value) -> Result<Value> {
        let enabled = p["enabled"].as_bool().context("Invalid enabled")?;
        let workspace = p.get("workspace").filter(|w| w.is_object()).context("Invalid workspace")?;
        anyhow::ensure!(
            p.as_object().is_some_and(|o| o.keys().all(|k| k == "workspace" || k == "enabled")),
            "Invalid params"
        );
        let previous = self.shell.off_peak_policy;
        for id in self.sessions.keys() {
            self.shell.off_peak.entry(id.clone()).or_insert(previous);
        }
        self.shell.off_peak_policy = enabled;
        Ok(json!({"workspace": workspace, "enabled": enabled}))
    }
    /// 会话创建：`offPeakToolEnabled === true` 或进程级结论。
    pub(super) fn fix_off_peak(&mut self, id: &str, requested: bool) {
        let enabled = requested || self.shell.off_peak_policy;
        self.shell.off_peak.insert(id.to_owned(), enabled);
    }
    /// 本会话是否注册 OffPeak 工具（首次读取时按当前进程级结论固化）。
    pub(super) fn off_peak_enabled(&mut self, id: &str) -> bool {
        let policy = self.shell.off_peak_policy;
        *self.shell.off_peak.entry(id.to_owned()).or_insert(policy)
    }
}

/// TS includeOffPeak：未开启或子代理（subagent_child）时不注册 OffPeak 工具；
/// TS buildTurnDisallowedTools：闲时受限轮（含只靠禁用列表哨兵判定的）隐藏 OffPeak 受限工具。
/// 注册面：会话未开 OffPeak 工具或子代理时这些工具不注册（调用回 `Tool not found`）。
pub(super) fn retain_visible(definitions: &mut Vec<Value>, facts: &super::context::TurnFacts, child: bool) {
    definitions.retain(|d| {
        let name = d["function"]["name"].as_str().unwrap_or_default();
        !(off_peak::TOOLS.contains(&name) && (!facts.off_peak_tools || child))
    });
}

/// 闲时受限轮只对模型隐藏改动类工具：它们仍在注册表里，模型硬调时回闲时拒绝文案（TS 可见性过滤）。
pub(super) fn hide_restricted(definitions: &mut Vec<Value>, facts: &super::context::TurnFacts) {
    if facts.off_peak_restricted {
        definitions.retain(|d| !off_peak::MUTATION_TOOLS.contains(&d["function"]["name"].as_str().unwrap_or_default()));
    }
}

pub(super) async fn execute(
    name: &str,
    args: &Value,
    facts: &super::context::TurnFacts,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let turn = Turn {
        off_peak_turn: facts.off_peak_restricted,
        active_off_peak_task_id: facts.off_peak_task_id.clone(),
        session_id: sink.session_id.clone(),
    };
    let request = off_peak::run(name, args, &turn, |method, params| async move {
        let (reply, answer) = oneshot::channel();
        sink.send(Event::HostRequest { method: method.into(), params, reply })
            .await
            .map_err(|error| HostError { code: 0, message: error.to_string() })?;
        match answer.await {
            Ok(Ok(raw)) => Json::parse(&raw).ok_or(HostError { code: 0, message: "Invalid Host response".into() }),
            Ok(Err((code, message, _))) => Err(HostError { code, message }),
            Err(_) => Err(HostError { code: 0, message: "Session owner stopped before the Host replied".into() }),
        }
    });
    let outcome = tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        outcome = request => outcome,
    };
    match outcome {
        Ok((data, content)) => Ok(ToolOutput::new(content, data)),
        Err(error) => Err(anyhow!(error)),
    }
}
