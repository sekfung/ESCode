//! `commands` channel（TS `ServiceChannels.Commands` = `"commands"`；docs/specs/rust-u2-commands.md）。
//!
//! 本片只接 `getPrimaryUserCommandsDirectory`（1/6）；其余 5 个由 `respond` 回显式缺口并列出已实现集合。
//! 域实现全在 `escode_host_services::commands`，这里只做参数解包与形状整形。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::domains_task_support::first_object;
use crate::session::{Channel, handler};
use crate::wire::Wire;

pub(crate) fn commands_channel(home: PathBuf) -> Channel {
    let primary = {
        let home = Arc::new(home);
        handler(move |arg: Wire| {
            let (home, params) = (Arc::clone(&home), first_object(arg));
            Box::pin(async move {
                let agent_source = params["agentSource"].as_str();
                let path = escode_host_services::commands::primary_user_commands_directory(
                    &home,
                    agent_source,
                )
                .await
                .map_err(|error| ("Error".to_owned(), error))?;
                Ok(Wire::Object(json!({ "path": path })))
            })
        })
    };
    Channel::new(
        "commands",
        vec![("getPrimaryUserCommandsDirectory", primary)],
    )
}
