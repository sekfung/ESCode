//! `settings-sync` channel（TS `ServiceChannels.SettingsSync` = `"settings-sync"`；docs/specs/rust-u2-sync.md §3）。
//!
//! 六个成员全部接通：Claude `CLAUDE.md` 迁移、首启提示状态、多 agent 设置发现与导入。

use std::path::Path;
use std::sync::Arc;

use escode_host_services::settings_sync::SettingsSyncService;
use escode_host_services::skills::Compare;
use serde_json::Value;

use crate::channel::Channel;
use crate::domains_task_support::first_object;
use crate::session::{RpcError, handler_rpc};
use crate::wire::Wire;

fn failed(message: String) -> RpcError {
    RpcError::new("Error", message)
}

pub(crate) fn settings_sync_channel(home: &Path) -> Channel {
    let compare: Compare = Arc::new(crate::collation::compare_names);
    let service = Arc::new(SettingsSyncService::new(home.to_path_buf(), compare));
    let migration_status = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .agents_file_migration_status()
                    .await
                    .map(|status| Wire::Object(status.to_json()))
                    .map_err(failed)
            })
        })
    };
    let copy_agents_file = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                // TS `params.overwrite !== true`：只有严格布尔 true 才允许覆盖。
                let overwrite = params.get("overwrite").and_then(Value::as_bool) == Some(true);
                service
                    .copy_agents_file(overwrite)
                    .await
                    .map(|result| Wire::Object(result.to_json()))
                    .map_err(failed)
            })
        })
    };
    let first_run_state = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .first_run_prompt_state()
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let mark_first_run_handled = {
        let service = Arc::clone(&service);
        handler_rpc(move |_arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                service
                    .mark_first_run_prompt_handled()
                    .await
                    .map(|()| Wire::Undefined)
                    .map_err(failed)
            })
        })
    };
    let detect = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .detect(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    let import_selected = {
        let service = Arc::clone(&service);
        handler_rpc(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service
                    .import_selected(&params)
                    .await
                    .map(Wire::Object)
                    .map_err(failed)
            })
        })
    };
    Channel::new(
        "settings-sync",
        vec![
            ("getClaudeAgentsFileMigrationStatus", migration_status),
            ("copyClaudeAgentsFileToEscodeAgentsFile", copy_agents_file),
            ("getFirstRunPromptState", first_run_state),
            ("markFirstRunPromptHandled", mark_first_run_handled),
            ("detect", detect),
            ("importSelected", import_selected),
        ],
    )
}
