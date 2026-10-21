//! Engine 的协议请求分派（一个方法名对应一个处理函数；状态在 engine.rs）。
use super::engine::Engine;
use crate::{
    contract::{Output, StorageCommitFailure},
    domain::protocol::{Request, rpc_error},
};
use anyhow::Result;
use serde_json::json;

impl Engine {
    pub(super) async fn request(&mut self, request: Request, output: &Output) -> Result<()> {
        self.outbox.clear();
        // 故障注入（仅 `fault-injection` feature，测试构建）：Host 崩溃隔离用例需要一个确定的 engine
        // panic 点；engine 循环的 catch_panic 把它转成 serve 错误，实例按正常收尾路径退出
        // （docs/specs/rust-u2-host-resilience.md §2.1）。
        #[cfg(feature = "fault-injection")]
        if request.method == "test/panic" {
            panic!("fault-injection: test/panic");
        }
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
        if super::wiki_requests::WIKI_METHODS.contains(&request.method.as_str()) {
            if let Err(error) = self.start_wiki_request(&request) {
                output
                    .send(vec![rpc_error(&request.id, -32602, &error.to_string())])
                    .await?;
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
            "v4/command" => match serde_json::from_value::<crate::domain::protocol::Command>(
                request.params.clone(),
            ) {
                Ok(command) => {
                    // accepted 结算需要命令的会话身份：createSession 用结果里的新会话、其余用信封 sessionId
                    // （TS v4-gateway accepted 结算同口径，差分账本 C-2）。
                    let envelope_session = command.session_id.clone();
                    let envelope_kind = command.kind.clone();
                    let ack = self.command(command).await;
                    ack.map(|mut ack| {
                        if ack["status"] == "accepted" {
                            // 物化期（C-1）已解析偏好时带上 memoryEnabled；未解析（旧 Host/超时）不带。
                            let target = if envelope_kind == "createSession" {
                                ack["result"]["sessionId"].as_str().map(str::to_owned)
                            } else {
                                envelope_session.clone()
                            };
                            if let Some(prefs) = target
                                .as_deref()
                                .and_then(|id| self.shell.cached(id, "runtime-materialization"))
                            {
                                ack["memoryEnabled"] = (prefs["memoryEnabled"] == true).into();
                            }
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
            // 修复原因：分支内的 `?` 会让 request() 本身返回 Err，serve 循环随之退出、进程 exit 1；
            // 参数校验失败只能作为本请求的 RPC 错误返回（见 docs/specs/rust-plugins.md「请求级错误隔离」）。
            "v4/conversation/usage" => {
                match request.params["sessionId"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    Some(session) => {
                        self.store
                            .usage(serde_json::json!({ "op": "task", "sessionId": session }))
                            .await
                    }
                    None => Err(anyhow::anyhow!("sessionId is required")),
                }
            }
            // 后台 Bash 详情的输出尾窗：观察查询，不恢复冷会话（TS readBackgroundBashOutputFromOwner）。
            "v4/conversation/backgroundBashOutput" => {
                let session = request.params["sessionId"].as_str();
                let work = request.params["workId"].as_str().filter(|w| !w.is_empty());
                match (session, work) {
                    (None, _) => Err(anyhow::anyhow!("sessionId is required")),
                    (_, None) => Err(anyhow::anyhow!("workId is required")),
                    (Some(session), Some(work)) => {
                        Ok(self.tools.background_bash_output(session, work).await)
                    }
                }
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
            // 修复原因：plugins/* 是控制面方法。Host 的插件管理进程固定以 ~/.escode/plugin-workspace 为 cwd，
            // 真实 workspace 由协议参数 `workspace.workspacePath` 给出（与 Node 一致，不比对 runtime 自身
            // workspace）。旧实现在这里 validate_workspace()? 必然不匹配，且 `?` 直接结束进程，Host 因此把
            // Rust 判为「就绪前失败」（docs/specs/rust-plugins.md「插件控制面的 workspace 语义」）。
            // App 插件页读面（docs/specs/rust-plugins.md 第 1 期）：workspace 级、无会话。
            "plugins/list" => self.tools.plugin_list(&request.params).await,
            // 已启用插件的 MCP App 面板入口（docs/specs/rust-mcp-ui.md）。
            "plugins/listUiSurfaces" => self.tools.plugin_ui_surfaces(&request.params).await,
            // 插件页开关（第 2 期）：落盘 enabledPlugins，新会话按新配置装载插件。
            "plugins/setEnabled" => self.tools.plugin_set_enabled(&request.params).await,
            // 插件选项面（第 4 期）：写 options / 恢复继承。
            "plugins/configure" => {
                self.tools
                    .plugin_configure(&request.params, request.raw_params.as_deref())
                    .await
            }
            "plugins/resetConfig" => self.tools.plugin_reset_config(&request.params).await,
            // 市场删除（写面 W4）；安装 / 更新 / 市场增刷走后台作业（plugin_jobs.rs）。
            "plugins/marketplace/remove" => {
                self.tools
                    .plugin_operation(&request.method, &request.params, &Default::default())
                    .await
            }
            "plugins/cancelOperation" => self.cancel_plugin_operation(&request.params),
            // 插件卸载 / 恢复内置（写面 W1a）。
            "plugins/uninstall" => self.tools.plugin_uninstall(&request.params).await,
            "plugins/restoreBuiltin" => self.tools.plugin_restore_builtin(&request.params).await,
            // 插件页概览（第 3 期）：市场摘要 + 目录条目 + 已安装记录 + 可恢复内置。
            "plugins/overview" => self.tools.plugin_overview(&request.params).await,
            // 对话 Picker 的插件引用目录（第 3 期）：session 冻结身份 / workspace 现算。
            "plugins/referenceCatalog" | "plugins/referenceCatalogWithCategory" => {
                self.plugin_reference_catalog(&request.method, &request.params)
                    .await
            }
            "v4/commands/query" => self.query_acks(&request.params).await,
            "session/list" => self.list_sessions(&request.params).await,
            "session/debug" => self.read_session_debug(&request.params).await,
            "session/subagents" => self.subagents_query(&request.params).await,
            "skills/referenceCatalog" => self.skill_catalog(&request.params).await,
            // 已保存工作流的 GUI 中枢（docs/specs/rust-dynamic-workflow.md 第 2 期）：
            // workspace 级、无会话；`workflows/runs` 是第 4 期的 journal。
            "workflows/list"
            | "workflows/get"
            | "workflows/updateMeta"
            | "workflows/delete"
            | "workflows/move"
            | "workflows/runs"
            | "workflows/save"
            | "workflows/forRun" => {
                let op = request.method.trim_start_matches("workflows/").to_owned();
                if op == "runs" {
                    self.saved_workflow_runs(&request.params).await
                } else if op == "save" || op == "forRun" {
                    self.saved_workflow_from_run(&op, &request.params).await
                } else {
                    self.saved_workflow_op(&op, &request.params).await
                }
            }
            // 修复：普通建会话（定时任务首跑、task facade）原先也进导入路径而被拒；
            // 只有带 importedHistory 的 claudeCode 历史才是导入（docs/specs/rust-legacy-session-methods.md）。
            "session/create" if request.params["importedHistory"]["source"] == "claudeCode" => {
                self.import_claude_history(&request.params).await
            }
            "session/create" => self.legacy_create(&request.params).await,
            // legacy 会话操作：翻译为 V4 命令走同一写路径（task facade、desktop session service 仍在用）。
            "session/send" => self.legacy_send(&request.params).await,
            "session/setModel" => self.legacy_set_model(&request.params).await,
            "session/setThoughtLevel" => self.legacy_set_thought_level(&request.params).await,
            "session/setMode" => self.legacy_set_mode(&request.params).await,
            "session/close" => self.legacy_close(&request.params).await,
            "provider/updateAccountConfig" => self.update_account(&request.params).await,
            "workspace/hooks/trustGrant" => self.workspace_hook_trust_grant(&request.params).await,
            // MCP Apps（docs/specs/rust-mcp-ui.md）：实例、资源与页面调用。workspace 校验与其它
            // workspace 级方法一致；失败作为本请求的 RPC 错误返回，不结束 serve 循环。
            // MCP Apps 采样走 Engine（需要任务模型）；其余 mcp/ui* 走工具层。
            "mcp/uiSampling" => match self.validate_workspace(&request.params) {
                Ok(()) => self.sample_mcp_ui(&request.params).await,
                Err(error) => Err(error),
            },
            "mcp/uiCancelSampling" => match self.validate_workspace(&request.params) {
                Ok(()) => self.cancel_mcp_sampling(&request.params),
                Err(error) => Err(error),
            },
            "mcp/readResource"
            | "mcp/uiOpenInstance"
            | "mcp/uiValidateInstance"
            | "mcp/uiCloseInstance"
            | "mcp/uiReadResource"
            | "mcp/uiListResources"
            | "mcp/uiListResourceTemplates"
            | "mcp/uiSubscribeResource"
            | "mcp/uiUnsubscribeResource"
            | "mcp/uiCallTool"
            | "mcp/uiCancelCall" => match self.validate_workspace(&request.params) {
                Ok(()) => self.tools.mcp_ui(&request.method, &request.params).await,
                Err(error) => Err(error),
            },
            _ => self.query(&request.method, &request.params),
        };
        let storage_failed = result
            .as_ref()
            .err()
            .is_some_and(anyhow::Error::is::<StorageCommitFailure>);
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
