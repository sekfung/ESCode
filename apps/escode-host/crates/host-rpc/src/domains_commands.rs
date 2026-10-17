//! `commands` channel（TS `ServiceChannels.Commands` = `"commands"`；docs/specs/rust-u2-commands.md）。
//!
//! 已接 5/6：`getPrimaryUserCommandsDirectory` + 写面四法（write/update/delete/setEnabled）。
//! 未接 `list`（多来源发现 + 插件清单扫描），由 `respond` 回显式缺口并列出已实现集合。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::commands::write::CommandsWriteService;
use serde_json::json;

use crate::domains_task_support::first_object;
use crate::session::{Channel, handler};
use crate::wire::Wire;

fn failed(error: String) -> (String, String) {
    ("Error".to_owned(), error)
}

pub(crate) fn commands_channel(home: PathBuf) -> Channel {
    let home = Arc::new(home);
    let service = Arc::new(CommandsWriteService::new(home.as_ref().clone()));

    let primary = {
        let home = Arc::clone(&home);
        handler(move |arg: Wire| {
            let (home, params) = (Arc::clone(&home), first_object(arg));
            Box::pin(async move {
                let path = escode_host_services::commands::primary_user_commands_directory(
                    &home,
                    params["agentSource"].as_str(),
                )
                .await
                .map_err(failed)?;
                Ok(Wire::Object(json!({ "path": path })))
            })
        })
    };

    let write_command_file = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let answer = service.write_command_file(&params).await.map_err(failed)?;
                Ok(Wire::Object(answer))
            })
        })
    };
    let update_command_file = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let answer = service.update_command_file(&params).await.map_err(failed)?;
                Ok(Wire::Object(answer))
            })
        })
    };
    let delete_command_file = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.delete_command_file(&params).await.map_err(failed)?;
                Ok(Wire::Undefined)
            })
        })
    };
    let set_command_enabled = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.set_command_enabled(&params).await.map_err(failed)?;
                Ok(Wire::Undefined)
            })
        })
    };
    let mut methods = vec![("getPrimaryUserCommandsDirectory", primary)];
    methods.extend(vec![
        ("writeCommandFile", write_command_file),
        ("updateCommandFile", update_command_file),
        ("deleteCommandFile", delete_command_file),
        ("setCommandEnabled", set_command_enabled),
    ]);
    Channel::new("commands", methods)
}
