//! `hooks` channel（TS `ServiceChannels.Hooks` = `"hooks"`；docs/specs/rust-u2-hooks.md）。
//!
//! - `loadHooks` / `saveHooks`：Host 本地文件域；
//! - `grantWorkspaceHookTrust`：转发运行时 Agent authority（`workspace/hooks/trustGrant`），
//!   Host 不直接写 trust store。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::domains_task_support::first_object;
use crate::forward::{Extract, Shaped, runtime_call_shaped};
use crate::session::{Channel, RpcError, handler, handler_rpc};
use crate::wire::Wire;

pub(crate) fn hooks_channel(
    home: PathBuf,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    let service = Arc::new(escode_host_services::hooks::HooksService::new(home));

    let load = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                let params = first_object(arg);
                let result = service.load(&params).await?;
                Ok(Wire::Object(result))
            })
        })
    };
    let save = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let service = Arc::clone(&service);
            Box::pin(async move {
                let params = first_object(arg);
                service.save(&params).await?;
                Ok(Wire::Undefined)
            })
        })
    };
    let grant = handler_rpc(move |arg: Wire| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        Box::pin(async move {
            let params = first_object(arg);
            let bundle_digest = required(&params, "bundleDigest")?;
            let hook_declaration_digest = required(&params, "hookDeclarationDigest")?;
            let shaped = Shaped {
                method: "workspace/hooks/trustGrant",
                body: json!({
                    "bundleDigest": bundle_digest,
                    "hookDeclarationDigest": hook_declaration_digest,
                }),
                with_workspace: true,
                trace: None,
            };
            runtime_call_shaped(&instances, &options, params, shaped, Extract::Result).await
        })
    });

    Channel::new(
        "hooks",
        vec![
            ("loadHooks", load),
            ("saveHooks", save),
            ("grantWorkspaceHookTrust", grant),
        ],
    )
}

fn required(params: &serde_json::Value, key: &'static str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            RpcError::new(
                "InvalidRequest",
                format!("grantWorkspaceHookTrust requires {key}"),
            )
        })
}
