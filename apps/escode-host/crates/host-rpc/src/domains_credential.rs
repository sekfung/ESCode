//! `credential` channel（docs/specs/rust-u2-credential.md）。

use std::path::PathBuf;
use std::sync::Arc;

use escode_host_services::credential::{CredentialError, CredentialService};
use escode_host_services::source_server_provider_provisioning::is_provisioning_credential_key;
use serde_json::Value;

use crate::channel::Channel;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

type Service = Option<Arc<CredentialService>>;

fn rpc_error(error: CredentialError) -> RpcError {
    let rpc = RpcError::new("Error", error.message);
    match error.code {
        Some(code) => rpc.with("code", Value::from(code)),
        None => rpc,
    }
}

fn string_arg(arg: &Wire, index: usize) -> String {
    match arg {
        Wire::Array(items) => items.get(index),
        other if index == 0 => Some(other),
        _ => None,
    }
    .and_then(Wire::as_str)
    .unwrap_or_default()
    .to_owned()
}

fn method(
    service: &Service,
    run: impl Fn(
        Arc<CredentialService>,
        Wire,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Wire, CredentialError>> + Send>,
    > + Send
    + Sync
    + 'static,
) -> MethodHandler {
    let service = service.clone();
    let run = Arc::new(run);
    handler_rpc(move |arg: Wire| {
        let service = service.clone();
        let run = Arc::clone(&run);
        Box::pin(async move {
            let Some(service) = service else {
                return Err(RpcError::new(
                    "NotImplemented",
                    "credential needs dataBaseDir: Main must pass dataBaseDir in rpcListen.",
                ));
            };
            run(service, arg).await.map_err(rpc_error)
        })
    })
}

/// 组合根构造 `credential` channel；secret 与 TS 同源（`ESCODE_CREDENTIAL_SECRET` 或按平台/家目录/用户名派生）。
///
/// `provisioning_trigger`：allowlist 凭据保存/删除成功后通知 Provider Provisioning（S4-g）。
pub(crate) fn credential_channel(
    data_base_dir: Option<PathBuf>,
    provisioning_trigger: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Channel {
    let service: Service = data_base_dir.map(|base| {
        Arc::new(CredentialService::new(
            CredentialService::path(&base),
            &escode_cli_host::credential_cipher::credential_secret(),
        ))
    });
    Channel::new(
        "credential",
        vec![
            (
                "load",
                method(&service, |s, arg| {
                    Box::pin(async move {
                        let key = string_arg(&arg, 0);
                        Ok(match s.load(&key).await? {
                            Some(value) => Wire::Str(value),
                            None => Wire::Object(Value::Null),
                        })
                    })
                }),
            ),
            ("save", {
                let trigger = provisioning_trigger.clone();
                method(&service, move |s, arg| {
                    let trigger = trigger.clone();
                    Box::pin(async move {
                        let key = string_arg(&arg, 0);
                        s.save(&key, &string_arg(&arg, 1)).await?;
                        if is_provisioning_credential_key(&key)
                            && let Some(trigger) = trigger
                        {
                            trigger();
                        }
                        Ok(Wire::Undefined)
                    })
                })
            }),
            ("delete", {
                let trigger = provisioning_trigger;
                method(&service, move |s, arg| {
                    let trigger = trigger.clone();
                    Box::pin(async move {
                        let key = string_arg(&arg, 0);
                        s.delete(&key).await?;
                        if is_provisioning_credential_key(&key)
                            && let Some(trigger) = trigger
                        {
                            trigger();
                        }
                        Ok(Wire::Undefined)
                    })
                })
            }),
        ],
    )
}
