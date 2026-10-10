//! Engine 的协议请求分派（一个方法名对应一个处理函数；状态在 engine.rs）。
use super::engine::Engine;
use crate::{
    contract::{Output, StorageCommitFailure},
    domain::protocol::{Request, rpc_error},
};
use anyhow::{Context, Result};
use serde_json::json;

impl Engine {
    pub(super) async fn request(&mut self, request: Request, output: &Output) -> Result<()> {
        self.outbox.clear();
        if request.method == "mcp/list" {
            if let Err(error) = self.start_mcp_query(&request) {
                output
                    .send(vec![rpc_error(&request.id, -32602, &error.to_string())])
                    .await?;
            }
            return Ok(());
        }
        if matches!(
            request.method.as_str(),
            "workspace/generateText" | "provider/testModelConnectivity"
        ) {
            if let Err(error) = self.start_auxiliary(&request) {
                output
                    .send(vec![rpc_error(&request.id, -32602, &error.to_string())])
                    .await?;
            }
            return Ok(());
        }
        // 工作流 run 的取消 / 恢复 / 中枢直接启动要等工作流宿主，而宿主会反向请求本 actor：
        // 后台执行、完成后应答。本入口早返回，不走请求尾部的 outbox 冲刷，所以同步拒绝（如
        // session_busy）由处理函数带回，在这里就地发出。
        if request.method == "v4/command"
            && super::workflow_run_commands::is_workflow_run_command(&request.params)
        {
            match self.start_workflow_run_command(&request).await {
                Ok(Some(batch)) => output.send(batch).await?,
                Ok(None) => {}
                Err(error) => {
                    output
                        .send(vec![rpc_error(&request.id, -32602, &error.to_string())])
                        .await?
                }
            }
            return Ok(());
        }
        if super::plugin_jobs::PLUGIN_JOB_METHODS.contains(&request.method.as_str()) {
            if let Err(error) = self.start_plugin_job(&request) {
                output
                    .send(vec![rpc_error(&request.id, -32602, &error.to_string())])
                    .await?;
            }
            return Ok(());
        }
        self.refresh_slash_commands(&request.method).await;
        let result = match request.method.as_str() {
            "v4/command" => match serde_json::from_value(request.params.clone()) {
                Ok(command) => {
                    // 本地 TTFT 观测（TS LocalTtftRecorder.receive / admitted）：只观察，不改变命令结果。
                    let ttft_admitted = self.local_ttft_receive(&request.params);
                    let command_id = request.params["commandId"].as_str().unwrap_or_default().to_owned();
                    let ack = self.command(command).await;
                    ack.map(|mut ack| {
                        if !ttft_admitted {
                            ack["ttftExcluded"] = "capacity".into();
                        } else if ack["status"] == "accepted" {
                            self.local_ttft_admitted(&command_id);
                        }
                        ack
                    })
                }
                Err(_) => {
                    output
                        .send(vec![rpc_error(
                            &request.id,
                            -32602,
                            "Invalid command envelope",
                        )])
                        .await?;
                    return Ok(());
                }
            },
            "v4/conversation/subscribe" => self.subscribe(&request.params).await,
            "v4/conversation/resync" => self.resync(&request.params),
            "v4/conversation/unsubscribe" => self.unsubscribe(&request.params),
            "v4/connection/flow" => self.connection_flow(&request.params),
            "v4/attachment/begin"
            | "v4/attachment/chunk"
            | "v4/attachment/commit"
            | "v4/attachment/abort" => {
                self.attachment_upload(&request.method, &request.params)
                    .await
            }
            "v4/attachment/read"
            | "v4/attachment/previewSource"
            | "v4/conversation/attachmentRead"
            | "v4/conversation/attachmentStat"
            | "v4/conversation/rowsRange"
            | "v4/conversation/plans" => {
                self.conversation_query(&request.method, &request.params)
                    .await
            }
            "workspace/updateInteractionPreferences" => {
                self.interaction_preferences(&request.params).await
            }
            // 「完整保留模型 IO」偏好（docs/specs/rust-model-io.md 第 2 期）。
            "workspace/updateModelIoPreferences" => self.model_io_preferences(&request.params),
            "v4/conversation/fileChanges" => self.file_changes(&request.params).await,
            // 应用用量统计（TS getUsageStats：queryAppUsage + buildAppUsageSnapshot）。
            "v4/usage/stats" => self.usage_stats(&request.params).await,
            // 会话用量（TS getTaskTokenUsage → usage store queryTaskUsage）。
            "v4/conversation/usage" => {
                let session = request.params["sessionId"].as_str().filter(|s| !s.is_empty()).context("sessionId is required")?;
                self.store.usage(serde_json::json!({ "op": "task", "sessionId": session })).await
            }
            // 后台 Bash 详情的输出尾窗：观察查询，不恢复冷会话（TS readBackgroundBashOutputFromOwner）。
            "v4/conversation/backgroundBashOutput" => {
                let session = request.params["sessionId"].as_str().context("sessionId is required")?;
                let work = request.params["workId"].as_str().filter(|w| !w.is_empty()).context("workId is required")?;
                Ok(self.tools.background_bash_output(session, work).await)
            }
            // V4 工作流只读查询（run 枚举 / 事件 / 产物 / 工作区）：工作流宿主按 Node 网关应答。
            "v4/conversation/workflowRuns"
            | "v4/conversation/workflowRunEvents"
            | "v4/conversation/workflowRunArtifacts"
            | "v4/conversation/workflowRunArtifactData"
            | "v4/conversation/workflowRunArtifactRead"
            | "v4/conversation/workflowRunWorkspace"
            | "v4/conversation/workflowRunNodeResult" => {
                let method = request.method.trim_start_matches("v4/conversation/");
                self.tools.workflow_query(method, &request.params).await
            }
            "v4/conversation/fileRewindPreview" => self.rewind_preview(&request.params).await,
            "session/read" => self.read_cold_session(&request.params).await,
            // App 的会话/任务恢复入口（TS resumeSession + activateSessionForResume）。
            "session/resume" => self.resume_session(&request.params).await,
            // legacy 消息分页读（TS readMessages；App `readSessionMessages` 仍在用）。
            "session/messages" => self.read_messages(&request.params),
            // App 插件页读面（docs/specs/rust-plugins.md 第 1 期）：workspace 级、无会话。
            "plugins/list" => {
                self.validate_workspace(&request.params)?;
                self.tools.plugin_list(&request.params).await
            }
            // 插件页开关（第 2 期）：落盘 enabledPlugins，新会话按新配置装载插件。
            "plugins/setEnabled" => {
                self.validate_workspace(&request.params)?;
                self.tools.plugin_set_enabled(&request.params).await
            }
            // 插件选项面（第 4 期）：写 options / 恢复继承。
            "plugins/configure" => {
                self.validate_workspace(&request.params)?;
                self.tools
                    .plugin_configure(&request.params, request.raw_params.as_deref())
                    .await
            }
            "plugins/resetConfig" => {
                self.validate_workspace(&request.params)?;
                self.tools.plugin_reset_config(&request.params).await
            }
            // 市场删除（写面 W4）；安装 / 更新 / 市场增刷走后台作业（plugin_jobs.rs）。
            "plugins/marketplace/remove" => {
                self.validate_workspace(&request.params)?;
                self.tools
                    .plugin_operation(&request.method, &request.params, &Default::default())
                    .await
            }
            "plugins/cancelOperation" => self.cancel_plugin_operation(&request.params),
            // 插件卸载 / 恢复内置（写面 W1a）。
            "plugins/uninstall" => {
                self.validate_workspace(&request.params)?;
                self.tools.plugin_uninstall(&request.params).await
            }
            "plugins/restoreBuiltin" => {
                self.validate_workspace(&request.params)?;
                self.tools.plugin_restore_builtin(&request.params).await
            }
            // 插件页概览（第 3 期）：市场摘要 + 目录条目 + 已安装记录 + 可恢复内置。
            "plugins/overview" => {
                self.validate_workspace(&request.params)?;
                self.tools.plugin_overview(&request.params).await
            }
            // 对话 Picker 的插件引用目录（第 3 期）：session 冻结身份 / workspace 现算。
            "plugins/referenceCatalog" | "plugins/referenceCatalogWithCategory" => {
                self.plugin_reference_catalog(&request.method, &request.params)
                    .await
            }
            // 时钟探测不触达命令账本（TS queryCommands 的 clock 分支）。
            "v4/commands/query" if request.params["clock"] == true => {
                self.local_ttft_clock(&request.params, super::local_ttft::now())
            }
            "v4/commands/query" => self.query_acks(&request.params).await,
            "session/list" => self.list_sessions(&request.params).await,
            "session/subagents" => self.subagents_query(&request.params).await,
            "skills/referenceCatalog" => self.skill_catalog(&request.params).await,
            // 已保存工作流的 GUI 中枢（docs/specs/rust-dynamic-workflow.md 第 2 期）：
            // workspace 级、无会话；`workflows/runs` 是第 4 期的 journal。
            "workflows/list"
            | "workflows/get"
            | "workflows/updateMeta"
            | "workflows/delete"
            | "workflows/move"
            | "workflows/runs" => {
                let op = request.method.trim_start_matches("workflows/").to_owned();
                if op == "runs" {
                    self.saved_workflow_runs(&request.params).await
                } else {
                    self.saved_workflow_op(&op, &request.params).await
                }
            }
            // 修复：普通建会话（定时/闲时任务首跑、task facade）原先也进共享上下文导入而被拒；
            // 只有带 importedHistory 的才是导入（docs/specs/rust-legacy-session-methods.md）。
            "session/create" if request.params.get("importedHistory").is_some() => {
                self.import_shared_context(&request.params).await
            }
            "session/create" => self.legacy_create(&request.params).await,
            // legacy 会话操作：翻译为 V4 命令走同一写路径（Bots、task facade、desktop session service 仍在用）。
            "session/send" => self.legacy_send(&request.params).await,
            "session/setModel" => self.legacy_set_model(&request.params).await,
            "session/setThoughtLevel" => self.legacy_set_thought_level(&request.params).await,
            "session/setMode" => self.legacy_set_mode(&request.params).await,
            "session/close" => self.legacy_close(&request.params).await,
            "provider/updateAccountConfig" => self.update_account(&request.params).await,
            "workspace/hooks/trustGrant" => self.workspace_hook_trust_grant(&request.params).await,
            _ => self.query(&request.method, &request.params),
        };
        let storage_failed = result
            .as_ref()
            .err()
            .is_some_and(|e| e.is::<StorageCommitFailure>());
        let response = match result {
            Ok(value) => json!({"id":request.id,"result":value}),
            Err(error) => {
                self.outbox.clear();
                let message = error.to_string();
                rpc_error(
                    &request.id,
                    if message.starts_with("Unsupported method:") {
                        -32601
                    } else {
                        -32602
                    },
                    &message,
                )
            }
        };
        let mut batch = vec![];
        if request.id.is_some() {
            batch.push(response);
        }
        batch.append(&mut self.outbox);
        if !batch.is_empty() {
            output.send(batch).await?;
        }
        if storage_failed {
            return Err(StorageCommitFailure.into());
        }
        self.trim_resident().await?;
        Ok(())
    }
}
