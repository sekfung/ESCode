//! `submitBotGroupInput`（S12）：冷恢复屏障 → 附件上传 → admission 复检 → V4 sendText。
//! 从 `domains_task_bot.rs` 拆出以守住单文件行数上限。

use serde_json::{Value, json};

use super::domains_task_bot::{admission_key, observer, resolve_target, v4_target};
use crate::agent_v4::observe::conversation_snapshot;
use crate::domains_task_bot_upload::upload_attachments;
use crate::domains_task_state::{StateContext, method};
use crate::forward::RuntimePolicy;
use crate::host_command::{self, HostCommand};
use crate::session::{MethodHandler, RpcError};
use crate::wire::Wire;

pub(super) fn submit_input(context: &StateContext) -> MethodHandler {
    method(context, |context, params| async move {
        let target = resolve_target(&context, &params, "Group task workspace mismatch")?;
        let observer = observer(&context)?;
        let routing = v4_target(&target, &params);
        let key = admission_key(&target, &params);
        let version = context.bot_group.capture(&key);
        // 冷恢复屏障：先确保运行时会话已加载，再上传附件。
        conversation_snapshot(&observer, &routing).await?;
        let attachments = params["attachments"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let attachments = upload_attachments(
            &observer,
            &routing,
            &context.bot_group.connection_id,
            attachments,
        )
        .await?;
        let source = &params["source"];
        let threaded = source["threadId"].as_str().is_some();
        if threaded && !context.bot_group.is_current(&key, version) {
            return Err(RpcError::new(
                "Error",
                "Topic input was cancelled during preparation",
            ));
        }
        let mut delivery = json!({
            "provider": source["provider"],
            "botId": source["botId"],
            "providerUserId": source["chatId"],
            "chatType": "group",
        });
        if threaded {
            delivery["threadId"] = source["threadId"].clone();
            if let Some(root) = source["rootMessageId"].as_str() {
                delivery["rootMessageId"] = Value::from(root);
            }
        }
        let mut payload = json!({
            "text": params["content"],
            "attachments": attachments,
            "requestedDelivery": if threaded { "startNow" } else { "queue" },
            "botGroupSource": source,
            "botDeliveryTarget": delivery,
        });
        if let Some(quotes) = params
            .get("conversationQuotes")
            .filter(|value| !value.is_null())
        {
            payload["conversationQuotes"] = quotes.clone();
        }
        let mut command = HostCommand::new("sendText", Some(&target.task_id), payload);
        command.command_id = params["commandId"].as_str().map(str::to_owned);
        command.client_id = Some(format!(
            "bot:{}:{}:{}",
            source["botId"].as_str().unwrap_or_default(),
            source["chatId"].as_str().unwrap_or_default(),
            source["senderId"].as_str().unwrap_or_default()
        ));
        let envelope = host_command::envelope(command)?;
        let command_id = params["commandId"].as_str().unwrap_or_default().to_owned();
        let send = || async {
            match observer
                .call(
                    &routing,
                    RuntimePolicy::StartIfNeeded,
                    "v4/command",
                    envelope.clone(),
                )
                .await
            {
                Ok(called) => {
                    host_command::assert_ack_ok(
                        "sendText",
                        &called.result,
                        &format!("session={}", target.task_id),
                    )?;
                    Ok(called.result)
                }
                Err(error) => {
                    let query = observer
                        .call(
                            &routing,
                            RuntimePolicy::StartIfNeeded,
                            "v4/commands/query",
                            json!({"commands": [{"sessionId": target.task_id, "commandId": command_id}]}),
                        )
                        .await
                        .ok()
                        .and_then(|called| called.result["results"].as_array().cloned())
                        .and_then(|mut results| results.drain(..).next());
                    match query {
                        Some(result)
                            if !result["result"].is_null() && result["result"] != "unknown" =>
                        {
                            Ok(result["result"].clone())
                        }
                        _ => Err(error),
                    }
                }
            }
        };
        let ack = if threaded {
            let (current, lock) = context.bot_group.begin(&key);
            if current != version {
                return Err(RpcError::new(
                    "Error",
                    "Topic input was cancelled during preparation",
                ));
            }
            let _guard = lock.lock().await;
            if !context.bot_group.is_current(&key, version) {
                return Err(RpcError::new(
                    "Error",
                    "Topic input was cancelled during preparation",
                ));
            }
            send().await?
        } else {
            send().await?
        };
        Ok(Wire::Object(ack))
    })
}
