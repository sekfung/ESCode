//! Hooks 调用口（docs/specs/rust-hooks.md H1）：执行与聚合在工作流宿主里复用 TS 的 hook 运行器；这里只判定
//! 本会话是否可能配置了 hooks（避免无 hooks 的会话为每次工具调用拉起 Node 宿主），并把调用转给宿主。
use super::WorkspaceTools;
use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// 用户 / env 配置里有 hooks 事件，或任一启用插件声明了 hook 来源（`hooks/hooks.json` / manifest `hooks`）。
/// 只做存在性判定；确切配置（合并、matcher、enabled）由宿主按 TS 口径解析。
async fn configured(cwd: &Path) -> bool {
    // H3：会话 mailbox 内部 hooks 的灰度（TS isMessageEnabled）。
    if matches!(std::env::var("ZCODE_MESSAGE_ENABLED").as_deref(), Ok("1" | "true")) {
        return true;
    }
    let config = super::extension_config::load(cwd)
        .await
        .unwrap_or_else(|_| json!({}));
    let hooks = &config["hooks"];
    if hooks["enabled"] != false
        && hooks["events"].as_object().is_some_and(|events| {
            events
                .values()
                .any(|m| m.as_array().is_some_and(|m| !m.is_empty()))
        })
    {
        return true;
    }
    let plugins = super::extension_plugins::enabled(cwd, &config, &CancellationToken::new())
        .await
        .unwrap_or_default();
    plugins.iter().any(|p| {
        p.manifest.get("hooks").is_some() || p.root.join("hooks").join("hooks.json").is_file()
    })
}

impl WorkspaceTools {
    /// 执行一次 hook 事件（TS HookRunner.run）：返回 HookRunResult；本会话没有 hooks 时返回 None。
    pub(super) async fn run_hook_inner(
        &self,
        session: &str,
        mut input: Value,
        call_id: Option<&str>,
    ) -> Option<Value> {
        // TS runPreToolUseHooks 带工具元数据的风险与副作用范围（静态表，不含 Bash 只读降级）。
        if input["hookEventName"] == "PreToolUse" || input["hookEventName"] == "PermissionRequest" {
            let table: Value =
                serde_json::from_str(include_str!("tool_capabilities.json")).unwrap_or_default();
            let entry = &table[input["toolName"].as_str().unwrap_or_default()];
            input["riskLevel"] = entry["riskLevel"].clone();
            input["sideEffectScope"] = entry["sideEffectScope"].clone();
        }
        let present = {
            let mut presence = self.hook_presence.lock().await;
            match presence.get(session) {
                Some(present) => *present,
                None => {
                    let present = configured(&self.cwd).await;
                    presence.insert(session.to_owned(), present);
                    present
                }
            }
        };
        if !present {
            return None;
        }
        let params = json!({
            "session": session, "cwd": self.workspace_path.to_string_lossy(), "input": input, "callId": call_id,
        });
        match self.workflow_host.request("hooks.run", params).await {
            Ok(result) if result["configured"] == true => Some(result),
            Ok(_) => {
                // 宿主按 TS 口径确认本会话没有 hooks：之后不再拉起宿主。
                self.hook_presence
                    .lock()
                    .await
                    .insert(session.to_owned(), false);
                None
            }
            Err(_) => None,
        }
    }
}
